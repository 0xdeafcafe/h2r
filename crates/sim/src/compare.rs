//! A/B comparison of a batch's reports (docs/runtime/sim.md §7).
//!
//! Each run gives one sample per arm: the mean over that arm's bots in the
//! run (per bot, so 2 v 2 and 1 v 1 compare). Per arm: the mean and the
//! standard deviation over runs. Between arms: B - A with a Welch 95%
//! interval.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::metrics::{Report, Winner};

/// The metrics compared, in table order. The last six are there when the
/// reports have them: the time to kill (seconds) and the aim motion (the
/// mean error in radians, the flick overshoot ratio, the settle time in
/// seconds, the stick travel a tick and its reversals a second;
/// docs/runtime/sim.md §6).
pub const METRICS: [&str; 15] = [
    "kills",
    "deaths",
    "kd",
    "accuracy",
    "damage_dealt",
    "score",
    "stuck_s",
    "grenades",
    "win",
    "ttk_s",
    "aim_error",
    "overshoot",
    "settle_s",
    "jitter",
    "reversals",
];

/// One arm's numbers in one run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ArmSample {
    pub run: String,
    pub arm: String,
    pub bots: usize,
    pub values: BTreeMap<String, f64>,
}

/// What a report's arms did. Players with no arm (no batch, or not our
/// bots) are left out.
pub fn samples(run: &str, r: &Report) -> Vec<ArmSample> {
    let mut by_arm: BTreeMap<String, Vec<&crate::metrics::PlayerReport>> = BTreeMap::new();
    for p in &r.players {
        if let Some(a) = &p.arm {
            by_arm.entry(a.clone()).or_default().push(p);
        }
    }
    let versus = by_arm.len() >= 2;
    let winner_arm: Option<String> = match &r.result.winner {
        Some(Winner::Player(i)) => r.player(*i).and_then(|p| p.arm.clone()),
        Some(Winner::Team(t)) => {
            let arms: Vec<&String> = r
                .players
                .iter()
                .filter(|p| p.team == Some(*t))
                .filter_map(|p| p.arm.as_ref())
                .collect();
            match arms.first() {
                Some(a) if arms.iter().all(|b| b == a) => Some((*a).clone()),
                _ => None,
            }
        }
        None => None,
    };
    by_arm
        .into_iter()
        .map(|(arm, ps)| {
            let n = ps.len() as f64;
            let mean = |f: &dyn Fn(&crate::metrics::PlayerReport) -> f64| {
                ps.iter().map(|p| f(p)).sum::<f64>() / n
            };
            let shots: u64 = ps.iter().map(|p| p.shots).sum();
            let hits: u64 = ps.iter().map(|p| p.hits).sum();
            let kills: f64 = ps.iter().map(|p| f64::from(p.kills)).sum();
            let deaths: f64 = ps.iter().map(|p| f64::from(p.deaths)).sum();
            let mut v = BTreeMap::new();
            v.insert("kills".into(), mean(&|p| f64::from(p.kills)));
            v.insert("deaths".into(), mean(&|p| f64::from(p.deaths)));
            v.insert("kd".into(), kills / deaths.max(1.0));
            v.insert(
                "accuracy".into(),
                if shots > 0 {
                    hits as f64 / shots as f64
                } else {
                    0.0
                },
            );
            v.insert("damage_dealt".into(), mean(&|p| p.damage_dealt));
            v.insert("score".into(), mean(&|p| f64::from(p.score)));
            v.insert("stuck_s".into(), mean(&|p| p.stuck_ticks as f64 / 30.0));
            v.insert("grenades".into(), mean(&|p| p.grenades as f64));
            // Each of these is a mean over what it counts (kills with a
            // time, tracked views, flicks), pooled over the arm's bots.
            let mut pooled =
                |name: &str, f: &dyn Fn(&crate::metrics::PlayerReport) -> (f64, f64)| {
                    let (sum, n) = ps
                        .iter()
                        .map(|p| f(p))
                        .fold((0.0, 0.0), |a, b| (a.0 + b.0 * b.1, a.1 + b.1));
                    if n > 0.0 {
                        v.insert(name.into(), sum / n);
                    }
                };
            pooled("ttk_s", &|p| (p.ttk_ticks / 30.0, p.ttk_kills as f64));
            let aim = |p: &crate::metrics::PlayerReport,
                       f: &dyn Fn(&crate::metrics::AimReport) -> (f64, u64)| {
                p.aim.as_ref().map_or((0.0, 0.0), |a| {
                    let (value, n) = f(a);
                    (value, n as f64)
                })
            };
            pooled("aim_error", &|p| {
                aim(p, &|a| (a.mean_error, a.tracked_ticks))
            });
            pooled("overshoot", &|p| aim(p, &|a| (a.overshoot_ratio, a.flicks)));
            pooled("settle_s", &|p| {
                aim(p, &|a| (a.settle_ticks / 30.0, a.settled))
            });
            pooled("jitter", &|p| {
                aim(p, &|a| (a.stick_jitter, a.tracked_ticks))
            });
            pooled("reversals", &|p| {
                aim(p, &|a| (a.reversals_per_s, a.tracked_ticks))
            });
            // A win means something only when both arms played the match.
            if versus {
                let w = match &winner_arm {
                    Some(a) if *a == arm => 1.0,
                    Some(_) => 0.0,
                    None => 0.5,
                };
                v.insert("win".into(), w);
            }
            ArmSample {
                run: run.into(),
                arm,
                bots: ps.len(),
                values: v,
            }
        })
        .collect()
}

/// Mean, standard deviation and count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stat {
    pub n: usize,
    pub mean: f64,
    pub sd: f64,
}

impl Stat {
    pub fn of(xs: &[f64]) -> Stat {
        let n = xs.len();
        if n == 0 {
            return Stat::default();
        }
        let mean = xs.iter().sum::<f64>() / n as f64;
        let sd = if n > 1 {
            (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
        } else {
            0.0
        };
        Stat { n, mean, sd }
    }
}

/// B - A for one metric.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Diff {
    pub diff: f64,
    /// The 95% interval's half width (Welch); `None` with fewer than two
    /// runs an arm.
    pub ci95: Option<f64>,
    /// The interval leaves out 0.
    pub significant: bool,
}

/// Two-sided 97.5% Student t quantiles for 1..=30 degrees of freedom.
const T975: [f64; 30] = [
    12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
    2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
    2.052, 2.048, 2.045, 2.042,
];

fn t975(df: f64) -> f64 {
    if !df.is_finite() || df > 30.0 {
        return 1.96;
    }
    let i = (df.floor().max(1.0) as usize).min(30);
    T975[i - 1]
}

/// Welch's difference of means, `b - a`.
pub fn welch(a: Stat, b: Stat) -> Diff {
    let diff = b.mean - a.mean;
    if a.n < 2 || b.n < 2 {
        return Diff {
            diff,
            ci95: None,
            significant: false,
        };
    }
    let va = a.sd * a.sd / a.n as f64;
    let vb = b.sd * b.sd / b.n as f64;
    let se2 = va + vb;
    if se2 == 0.0 {
        return Diff {
            diff,
            ci95: Some(0.0),
            significant: diff != 0.0,
        };
    }
    let df = se2 * se2 / (va * va / (a.n - 1) as f64 + vb * vb / (b.n - 1) as f64);
    let ci95 = t975(df) * se2.sqrt();
    Diff {
        diff,
        ci95: Some(ci95),
        significant: diff.abs() > ci95,
    }
}

/// Each observation is one run pair's mean per logical seat. A seat
/// contributes once under each transport, including when its player index changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeatComparison {
    pub pairs: usize,
    pub excluded: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exclusion_reasons: BTreeMap<String, String>,
    pub local: BTreeMap<String, Stat>,
    pub phantom: BTreeMap<String, Stat>,
    pub local_minus_phantom: BTreeMap<String, Stat>,
}

const SEAT_METRICS: [&str; 4] = ["shots", "opposing_hits", "damage_dealt", "damage_taken"];

fn seat_values(r: &Report) -> Option<BTreeMap<u8, (crate::config::SeatKind, [f64; 4])>> {
    let specs = r.match_["bot"].as_array()?;
    let mut seats = BTreeMap::new();
    for p in &r.players {
        let seat = p.seat?;
        let kind = p.kind?;
        let spec = specs.get(usize::from(seat))?;
        if serde_json::to_value(kind).ok()? != spec["kind"] || !p.bot {
            return None;
        }
        if seats
            .insert(
                seat,
                (
                    kind,
                    [
                        p.shots as f64,
                        p.opposing_hits as f64,
                        p.damage_dealt,
                        p.damage_taken,
                    ],
                ),
            )
            .is_some()
        {
            return None;
        }
    }
    (seats.len() == specs.len() && !seats.is_empty()).then_some(seats)
}

fn seat_comparison(reports: &[(String, Report)]) -> Option<SeatComparison> {
    use crate::{config::SeatKind, metrics::End};
    let mut pairs = BTreeMap::<(&str, u64, String), [Vec<&Report>; 2]>::new();
    for (id, r) in reports {
        if let (Some(leg), Some(map), Some(seed)) = (
            r.match_["seats_swapped"].as_bool(),
            r.match_["map"].as_str(),
            r.match_["seed"].as_u64(),
        ) {
            // A planned run's ID includes any game type and skill axes.
            // Older callers can still supply unrelated report IDs.
            let stem = if leg {
                id.strip_suffix("-swapped")
            } else {
                Some(id.as_str())
            };
            let seed_suffix = format!("-s{seed}");
            let pair_id = stem
                .filter(|stem| stem.ends_with(seed_suffix.as_str()))
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{map}{seed_suffix}"));
            pairs.entry((map, seed, pair_id)).or_default()[usize::from(leg)].push(r);
        }
    }
    if pairs.is_empty() {
        return None;
    }
    let mut result = SeatComparison::default();
    let mut samples = Vec::new();
    for ((_, _, id), legs) in pairs {
        let pair = (|| {
            let ([a], [b]) = (legs[0].as_slice(), legs[1].as_slice()) else {
                return Err(format!(
                    "{} original and {} swapped reports",
                    legs[0].len(),
                    legs[1].len()
                ));
            };
            for (name, r) in [("original", a), ("swapped", b)] {
                if r.ticks == 0 || !matches!(r.result.end, End::GameOver | End::TickLimit) {
                    let mut why = format!(
                        "{name} leg ended {} with {} ticks and {} players",
                        r.result.end.name(),
                        r.ticks,
                        r.players.len()
                    );
                    if let Some(note) = r
                        .notes
                        .iter()
                        .find(|n| n.starts_with("waiting for player/unit bindings"))
                    {
                        let _ = write!(why, " ({note})");
                    }
                    return Err(why);
                }
            }
            // Everything except transport and the leg marker must match,
            // including seat order, resolved tuning, teams and game limits.
            let mut matched = b.match_.clone();
            matched["seats_swapped"] = false.into();
            for bot in matched["bot"]
                .as_array_mut()
                .ok_or("missing swapped roster")?
            {
                let kind: SeatKind = serde_json::from_value(bot["kind"].clone())
                    .map_err(|_| "invalid swapped seat kind")?;
                bot["kind"] = serde_json::to_value(kind.swapped()).map_err(|e| e.to_string())?;
            }
            if matched != a.match_ {
                return Err("match settings differ after swapping seat kinds".into());
            }
            let original = seat_values(a).ok_or("invalid or incomplete original roster")?;
            let swapped = seat_values(b).ok_or("invalid or incomplete swapped roster")?;
            let mut totals = [[0.; 4]; 2];
            let count = original.len() as f64;
            for (seat, (kind, x)) in original {
                let (other, y) = swapped.get(&seat).ok_or("missing logical seat")?;
                if *other != kind.swapped() {
                    return Err(format!("seat {seat} did not change kind"));
                }
                let (local, phantom) = if kind == SeatKind::Local {
                    (x, *y)
                } else {
                    (*y, x)
                };
                for i in 0..4 {
                    totals[0][i] += local[i] / count;
                    totals[1][i] += phantom[i] / count;
                }
            }
            Ok(totals)
        })();
        match pair {
            Ok(values) => samples.push(values),
            Err(reason) => {
                result.exclusion_reasons.insert(id.clone(), reason);
                result.excluded.push(id);
            }
        }
    }
    result.pairs = samples.len();
    if result.pairs > 0 {
        for (i, name) in SEAT_METRICS.iter().enumerate() {
            for (out, side) in [(&mut result.local, 0), (&mut result.phantom, 1)] {
                out.insert(
                    (*name).into(),
                    Stat::of(&samples.iter().map(|p| p[side][i]).collect::<Vec<_>>()),
                );
            }
            result.local_minus_phantom.insert(
                (*name).into(),
                Stat::of(
                    &samples
                        .iter()
                        .map(|p| p[0][i] - p[1][i])
                        .collect::<Vec<_>>(),
                ),
            );
        }
    }
    Some(result)
}

/// A batch's comparison: `compare.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat_swap: Option<SeatComparison>,
    pub runs: usize,
    /// Arm names, A first.
    pub arms: Vec<String>,
    /// Per arm, per metric.
    pub stats: BTreeMap<String, BTreeMap<String, Stat>>,
    /// Per metric, the second arm less the first (with two arms).
    pub diff: BTreeMap<String, Diff>,
    pub samples: Vec<ArmSample>,
    /// Runs with no report, or none that parsed.
    #[serde(default)]
    pub missing: Vec<String>,
    /// The same statistics over each skill's runs alone, when the runs
    /// are of more than one skill (a batch's `skills`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_skill: BTreeMap<String, SkillComparison>,
}

/// One skill's runs of a comparison.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillComparison {
    pub runs: usize,
    pub stats: BTreeMap<String, BTreeMap<String, Stat>>,
    pub diff: BTreeMap<String, Diff>,
}

/// The skills in the order a table lists them.
const SKILLS: [&str; 4] = ["easy", "normal", "heroic", "legendary"];

/// The skill every bot of a report played at, if they all played at one.
fn skill_of(r: &Report) -> Option<&str> {
    let bots = r.match_["bot"].as_array()?;
    let first = bots.first()?["skill"].as_str()?;
    bots.iter()
        .all(|b| b["skill"].as_str() == Some(first))
        .then_some(first)
}

type ArmStats = BTreeMap<String, BTreeMap<String, Stat>>;

/// Per arm and metric over `samples`, and the second arm less the first.
fn summarise(names: &[String], samples: &[&ArmSample]) -> (ArmStats, BTreeMap<String, Diff>) {
    let mut stats = BTreeMap::new();
    for arm in names {
        let mut per = BTreeMap::new();
        for m in METRICS {
            let xs: Vec<f64> = samples
                .iter()
                .filter(|s| &s.arm == arm)
                .filter_map(|s| s.values.get(m).copied())
                .collect();
            if !xs.is_empty() {
                per.insert(m.to_string(), Stat::of(&xs));
            }
        }
        stats.insert(arm.clone(), per);
    }
    let mut diff = BTreeMap::new();
    if names.len() >= 2 {
        for m in METRICS {
            if let (Some(a), Some(b)) = (stats[&names[0]].get(m), stats[&names[1]].get(m)) {
                diff.insert(m.to_string(), welch(*a, *b));
            }
        }
    }
    (stats, diff)
}

/// Compares the reports of a batch, `(run id, report)`, with `arms` in
/// order (A first). Arms not named in `arms` follow in name order.
pub fn compare(arms: &[String], reports: &[(String, Report)], missing: Vec<String>) -> Comparison {
    let samples: Vec<ArmSample> = reports.iter().flat_map(|(id, r)| samples(id, r)).collect();
    let mut names: Vec<String> = arms.to_vec();
    for s in &samples {
        if !names.contains(&s.arm) {
            names.push(s.arm.clone());
        }
    }
    let all: Vec<&ArmSample> = samples.iter().collect();
    let (stats, diff) = summarise(&names, &all);
    let mut skills: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, r) in reports {
        if let Some(skill) = skill_of(r) {
            skills.entry(skill).or_default().push(id.as_str());
        }
    }
    let mut by_skill = BTreeMap::new();
    if skills.len() >= 2 {
        for (skill, ids) in skills {
            let of: Vec<&ArmSample> = samples
                .iter()
                .filter(|s| ids.contains(&s.run.as_str()))
                .collect();
            let (stats, diff) = summarise(&names, &of);
            by_skill.insert(
                skill.to_string(),
                SkillComparison {
                    runs: ids.len(),
                    stats,
                    diff,
                },
            );
        }
    }
    Comparison {
        seat_swap: seat_comparison(reports),
        runs: reports.len(),
        arms: names,
        stats,
        diff,
        samples,
        missing,
        by_skill,
    }
}

/// A table's rows: one a metric, a column an arm, then B - A.
fn rows(out: &mut String, arms: &[String], stats: &ArmStats, diff: &BTreeMap<String, Diff>) {
    for m in METRICS {
        if !arms
            .iter()
            .any(|a| stats.get(a).is_some_and(|s| s.contains_key(m)))
        {
            continue;
        }
        let _ = write!(out, "{m:<14}");
        for a in arms {
            match stats.get(a).and_then(|s| s.get(m)) {
                Some(s) => {
                    let _ = write!(
                        out,
                        "{:>22}",
                        format!("{:.3} ± {:.3} (n={})", s.mean, s.sd, s.n)
                    );
                }
                None => {
                    let _ = write!(out, "{:>22}", "-");
                }
            }
        }
        if let Some(d) = diff.get(m) {
            let ci = d
                .ci95
                .map_or_else(|| "?".to_string(), |c| format!("{c:.3}"));
            let mark = if d.significant { " *" } else { "" };
            let _ = write!(out, "{:>26}", format!("{:+.3} ± {ci}{mark}", d.diff));
        }
        out.push('\n');
    }
}

impl Comparison {
    /// The text table: one row per metric, a column per arm, then B - A.
    pub fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{} runs", self.runs);
        if !self.missing.is_empty() {
            let _ = writeln!(out, "no report: {}", self.missing.join(", "));
        }
        let _ = write!(out, "{:<14}", "metric");
        for a in &self.arms {
            let _ = write!(out, "{:>22}", a);
        }
        if self.arms.len() >= 2 {
            let _ = write!(
                out,
                "{:>26}",
                format!("{} - {}", self.arms[1], self.arms[0])
            );
        }
        out.push('\n');
        rows(&mut out, &self.arms, &self.stats, &self.diff);
        let others = self
            .by_skill
            .keys()
            .map(String::as_str)
            .filter(|k| !SKILLS.contains(k));
        let listed = SKILLS.iter().copied().chain(others);
        for skill in listed {
            if let Some(part) = self.by_skill.get(skill) {
                let _ = writeln!(out, "\n{skill} ({} runs)", part.runs);
                rows(&mut out, &self.arms, &part.stats, &part.diff);
            }
        }
        if self.arms.len() >= 2 {
            out.push_str("± is the sd per arm, and the 95% interval (Welch) for the difference; * leaves out 0\n");
        }
        if let Some(pair) = &self.seat_swap {
            let _ = writeln!(out, "Seat swap ({} complete pairs)", pair.pairs);
            let _ = writeln!(
                out,
                "{:<16}{:>12}{:>12}{:>18}",
                "per seat/game", "local", "phantom", "local - phantom"
            );
            for (metric, gap) in &pair.local_minus_phantom {
                let _ = writeln!(
                    out,
                    "{metric:<16}{:>12.3}{:>12.3}{:>+18.3}",
                    pair.local[metric].mean, pair.phantom[metric].mean, gap.mean
                );
            }
            for id in &pair.excluded {
                let reason = pair
                    .exclusion_reasons
                    .get(id)
                    .map_or("incomplete pair", String::as_str);
                let _ = writeln!(out, "Excluded {id}: {reason}");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{End, MatchResult, PlayerReport, Wall};

    fn bot(
        index: u8,
        arm: &str,
        team: i32,
        kills: i32,
        deaths: i32,
        shots: u64,
        hits: u64,
    ) -> PlayerReport {
        PlayerReport {
            index,
            name: format!("b{index}"),
            bot: true,
            port: Some(index),
            arm: Some(arm.into()),
            team: Some(team),
            kills,
            deaths,
            score: kills,
            shots,
            hits,
            stuck_ticks: 60,
            ..PlayerReport::default()
        }
    }

    #[test]
    fn midship_reports_identify_the_zero_tick_swapped_leg() {
        // The four-player seed 1 report and the empty swapped report from the batch.
        let [original, swapped]: [Report; 2] =
            serde_json::from_str(include_str!("compare/midship-pair.json")).unwrap();
        let reports = |swapped| {
            [
                ("midship-s1".into(), original.clone()),
                ("midship-s1-swapped".into(), swapped),
            ]
        };
        let c = compare(&[], &reports(swapped.clone()), vec![]);
        let pair = c.seat_swap.as_ref().unwrap();
        assert_eq!(pair.pairs, 0);
        assert_eq!(pair.excluded, ["midship-s1"]);
        let reason = &pair.exclusion_reasons["midship-s1"];
        assert!(reason.contains("swapped leg ended game_over with 0 ticks and 0 players"));
        assert!(reason.contains("Blue 1 (Local seat 1), Blue 2 (Local seat 3)"));
        assert!(c.table().contains(reason.as_str()));

        // Model a successful return leg using the same saved match block.
        let mut played = swapped;
        played.ticks = 9000;
        played.result.end = End::TickLimit;
        played.players = original.players.clone();
        played.players.reverse();
        for (index, p) in played.players.iter_mut().enumerate() {
            p.index = index as u8;
            p.kind = p.kind.map(crate::config::SeatKind::swapped);
            p.port =
                (p.kind == Some(crate::config::SeatKind::Local)).then_some(p.seat.unwrap() / 2);
            p.shots = 100;
        }
        let pair = compare(&[], &reports(played), vec![]).seat_swap.unwrap();
        assert_eq!(pair.pairs, 1);
        assert_eq!(pair.local["shots"].mean, (575. + 734. + 200.) / 4.);
        assert_eq!(pair.phantom["shots"].mean, (532. + 610. + 200.) / 4.);
    }

    #[test]
    fn paired_seats_use_only_complete_games_and_follow_logical_identity() {
        use crate::config::SeatKind::{Local, Phantom};
        let leg = |swapped, seed| {
            let (a, b) = if swapped {
                (Phantom, Local)
            } else {
                (Local, Phantom)
            };
            let mut r = report(vec![], None);
            r.ticks = 1000;
            r.match_ = serde_json::json!({"seats_swapped":swapped,"map":"midship","seed":seed,"bot":[{"kind":a},{"kind":b}]});
            for (seat, kind, shots) in [
                (0, a, if swapped { 9 } else { 30 }),
                (1, b, if swapped { 10 } else { 5 }),
            ] {
                r.players.push(PlayerReport {
                    seat: Some(seat),
                    index: if swapped { 1 - seat } else { seat },
                    kind: Some(kind),
                    bot: true,
                    shots,
                    opposing_hits: shots / 2,
                    damage_dealt: shots as f64,
                    damage_taken: 2. * shots as f64,
                    ..Default::default()
                });
            }
            (format!("{seed}-{swapped}"), r)
        };
        let mut invalid = leg(false, 3);
        invalid.1.players.pop();
        let mut uneven = leg(true, 4);
        uneven.1.match_["bot"][0]["team"] = 1.into();
        let reports = [
            uneven,
            leg(false, 4),
            leg(true, 1),
            leg(false, 1),
            leg(false, 2),
            invalid,
            leg(true, 3),
        ];
        let pair = compare(&[], &reports, vec![]).seat_swap.unwrap();
        assert_eq!(pair.pairs, 1);
        assert_eq!(pair.excluded, ["midship-s2", "midship-s3", "midship-s4"]);
        assert_eq!(pair.local["shots"].mean, 20.);
        assert_eq!(pair.phantom["shots"].mean, 7.);
        assert_eq!(pair.local_minus_phantom["shots"].mean, 13.);
        assert_eq!(pair.local_minus_phantom["damage_taken"].mean, 26.);
        assert_eq!(pair.local_minus_phantom["opposing_hits"].mean, 7.);

        let mut reports = Vec::new();
        for game_type in ["ctf", "juggernaut"] {
            for skill in ["easy", "heroic"] {
                for swapped in [false, true] {
                    if (game_type, skill, swapped) == ("ctf", "heroic", true) {
                        continue;
                    }
                    let (_, mut report) = leg(swapped, 1);
                    report.match_["game_type"] = game_type.into();
                    for bot in report.match_["bot"].as_array_mut().unwrap() {
                        bot["skill"] = skill.into();
                    }
                    let suffix = if swapped { "-swapped" } else { "" };
                    reports.push((format!("midship-{game_type}-{skill}-s1{suffix}"), report));
                }
            }
        }
        let pair = compare(&[], &reports, vec![]).seat_swap.unwrap();
        assert_eq!(pair.pairs, 3);
        assert_eq!(pair.excluded, ["midship-ctf-heroic-s1"]);
    }

    fn report(players: Vec<PlayerReport>, winner: Option<Winner>) -> Report {
        Report {
            format: crate::metrics::FORMAT.into(),
            match_: serde_json::Value::Null,
            result: MatchResult {
                end: End::GameOver,
                winner,
                teams: vec![],
            },
            ticks: 0,
            state_hashes_v1: Vec::new(),
            game_seconds: 0.0,
            wall: Wall::default(),
            players,
            kills: vec![],
            nav: "loaded".into(),
            notes: vec![],
        }
    }

    #[test]
    fn stats() {
        let s = Stat::of(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(s.n, 4);
        assert_eq!(s.mean, 2.5);
        assert!((s.sd - 1.290_994).abs() < 1e-5);
        assert_eq!(Stat::of(&[]), Stat::default());
        assert_eq!(Stat::of(&[5.0]).sd, 0.0);
    }

    #[test]
    fn welch_intervals() {
        let a = Stat::of(&[10.0, 11.0, 9.0, 10.0, 10.0]);
        let b = Stat::of(&[14.0, 15.0, 13.0, 14.0, 14.0]);
        let d = welch(a, b);
        assert_eq!(d.diff, 4.0);
        assert!(d.significant);
        let ci = d.ci95.unwrap();
        assert!(ci > 1.0 && ci < 1.1, "{ci}");
        let same = welch(a, a);
        assert!(!same.significant);
        let one = welch(Stat::of(&[1.0]), b);
        assert!(one.ci95.is_none() && !one.significant);
        assert_eq!(t975(1.0), 12.706);
        assert_eq!(t975(100.0), 1.96);
    }

    #[test]
    fn samples_per_arm() {
        let r = report(
            vec![
                bot(0, "base", 0, 4, 2, 100, 30),
                bot(1, "aggr", 1, 6, 4, 100, 20),
                bot(2, "base", 0, 2, 2, 100, 10),
                bot(3, "aggr", 1, 2, 4, 0, 0),
            ],
            Some(Winner::Team(1)),
        );
        let s = samples("r1", &r);
        assert_eq!(s.len(), 2);
        let base = s.iter().find(|s| s.arm == "base").unwrap();
        assert_eq!(base.bots, 2);
        assert_eq!(base.values["kills"], 3.0);
        assert_eq!(base.values["kd"], 1.5);
        assert_eq!(base.values["accuracy"], 0.2);
        assert_eq!(base.values["stuck_s"], 2.0);
        assert_eq!(base.values["win"], 0.0);
        let aggr = s.iter().find(|s| s.arm == "aggr").unwrap();
        assert_eq!(aggr.values["win"], 1.0);
        assert_eq!(aggr.values["kd"], 1.0);
    }

    #[test]
    fn ties_and_single_arms() {
        let tie = report(
            vec![bot(0, "a", 0, 1, 1, 0, 0), bot(1, "b", 1, 1, 1, 0, 0)],
            None,
        );
        assert!(samples("t", &tie).iter().all(|s| s.values["win"] == 0.5));
        // Separate mode: one arm a match, no win.
        let solo = report(
            vec![bot(0, "a", 0, 1, 0, 0, 0), bot(1, "a", 1, 0, 1, 0, 0)],
            Some(Winner::Player(0)),
        );
        let s = samples("s", &solo);
        assert_eq!(s.len(), 1);
        assert!(!s[0].values.contains_key("win"));
        // Not ours: no arm, no sample.
        let mut human = bot(0, "a", 0, 1, 0, 0, 0);
        human.arm = None;
        assert!(samples("h", &report(vec![human], None)).is_empty());
    }

    #[test]
    fn time_to_kill_and_aim_motion_pool_over_what_they_count() {
        use crate::metrics::AimReport;
        let mut a = bot(0, "code", 0, 4, 2, 100, 30);
        a.ttk_kills = 3;
        a.ttk_ticks = 60.0;
        a.aim = Some(AimReport {
            tracked_ticks: 300,
            mean_error: 0.10,
            flicks: 4,
            overshoot_ratio: 0.2,
            settled: 2,
            settle_ticks: 30.0,
            stick_jitter: 0.1,
            reversals_per_s: 2.0,
        });
        let mut b = bot(2, "code", 0, 1, 2, 100, 30);
        b.ttk_kills = 1;
        b.ttk_ticks = 20.0;
        b.aim = Some(AimReport {
            tracked_ticks: 100,
            mean_error: 0.02,
            flicks: 0,
            settled: 0,
            stick_jitter: 0.3,
            reversals_per_s: 6.0,
            ..AimReport::default()
        });
        // An arm with neither (an earlier report) has none of the rows.
        let old = bot(1, "net", 1, 2, 3, 50, 10);
        let s = samples("r", &report(vec![a, old, b], Some(Winner::Team(0))));
        let code = &s.iter().find(|s| s.arm == "code").unwrap().values;
        let near = |name: &str, want: f64| {
            assert!((code[name] - want).abs() < 1e-9, "{name}: {}", code[name]);
        };
        near("ttk_s", (3.0 * 2.0 + 1.0 * 20.0 / 30.0) / 4.0);
        near("aim_error", (300.0 * 0.10 + 100.0 * 0.02) / 400.0);
        near("overshoot", 0.2);
        near("settle_s", 1.0);
        near("jitter", (300.0 * 0.1 + 100.0 * 0.3) / 400.0);
        near("reversals", (300.0 * 2.0 + 100.0 * 6.0) / 400.0);
        let net = &s.iter().find(|s| s.arm == "net").unwrap().values;
        for name in [
            "ttk_s",
            "aim_error",
            "overshoot",
            "settle_s",
            "jitter",
            "reversals",
        ] {
            assert!(METRICS.contains(&name));
            assert!(!net.contains_key(name), "{name}");
        }
        assert_eq!(net["kills"], 2.0);
    }

    #[test]
    fn runs_of_several_skills_get_a_table_each() {
        let run = |skill: &str, kills: i32| {
            let mut r = report(
                vec![
                    bot(0, "code", 0, kills, 3, 10, 5),
                    bot(1, "net", 1, kills + 2, 3, 10, 5),
                ],
                Some(Winner::Player(1)),
            );
            r.match_ = serde_json::json!({"bot": [{"skill": skill}, {"skill": skill}]});
            r
        };
        let mut reports = Vec::new();
        for (skill, base) in [("easy", 1), ("legendary", 10)] {
            for i in 0..3 {
                reports.push((format!("{skill}-{i}"), run(skill, base + i)));
            }
        }
        let arms = ["code".to_string(), "net".to_string()];
        let c = compare(&arms, &reports, vec![]);
        assert_eq!(c.runs, 6);
        assert_eq!(c.stats["code"]["kills"].n, 6);
        assert_eq!(c.by_skill.len(), 2);
        let easy = &c.by_skill["easy"];
        assert_eq!(easy.runs, 3);
        assert_eq!(easy.stats["code"]["kills"].mean, 2.0);
        assert_eq!(easy.stats["net"]["kills"].mean, 4.0);
        assert_eq!(easy.diff["kills"].diff, 2.0);
        assert_eq!(c.by_skill["legendary"].stats["code"]["kills"].mean, 11.0);
        let t = c.table();
        let (easy_at, legendary_at) = (t.find("\neasy (3 runs)"), t.find("\nlegendary (3 runs)"));
        assert!(easy_at.is_some() && easy_at < legendary_at, "{t}");
        let back: Comparison = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.by_skill, c.by_skill);
        // One skill, a mixed lineup or no skills at all: the one table.
        let one = compare(&arms, &reports[..3], vec![]);
        assert!(one.by_skill.is_empty());
        assert!(!serde_json::to_string(&one).unwrap().contains("by_skill"));
        assert!(!one.table().contains("runs)"));
        let mut mixed = run("easy", 1);
        mixed.match_ = serde_json::json!({"bot": [{"skill": "easy"}, {"skill": "heroic"}]});
        assert_eq!(skill_of(&mixed), None);
        assert_eq!(skill_of(&run("heroic", 1)), Some("heroic"));
        assert_eq!(skill_of(&report(vec![], None)), None);
    }

    #[test]
    fn compares_runs() {
        let run = |k: i32| {
            report(
                vec![bot(0, "a", 0, k, 3, 10, 5), bot(1, "b", 1, k + 3, 3, 10, 5)],
                Some(Winner::Player(1)),
            )
        };
        let reports: Vec<(String, Report)> = (0..5).map(|i| (format!("r{i}"), run(i))).collect();
        let c = compare(&["a".into(), "b".into()], &reports, vec!["r9".into()]);
        assert_eq!(c.runs, 5);
        assert_eq!(c.arms, ["a", "b"]);
        assert_eq!(c.stats["a"]["kills"].n, 5);
        assert_eq!(c.diff["kills"].diff, 3.0);
        assert_eq!(c.diff["win"].diff, 1.0);
        let t = c.table();
        assert!(t.contains("5 runs"));
        assert!(t.contains("no report: r9"));
        assert!(t.contains("b - a"));
        assert!(t.contains("kills"));
        let back: Comparison = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.diff, c.diff);
    }
}
