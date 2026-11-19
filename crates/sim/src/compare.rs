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

/// The metrics compared, in table order.
pub const METRICS: [&str; 9] = [
    "kills",
    "deaths",
    "kd",
    "accuracy",
    "damage_dealt",
    "score",
    "stuck_s",
    "grenades",
    "win",
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

/// A batch's comparison: `compare.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
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
    let mut stats = BTreeMap::new();
    for arm in &names {
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
    Comparison {
        runs: reports.len(),
        arms: names,
        stats,
        diff,
        samples,
        missing,
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
        for m in METRICS {
            if !self
                .arms
                .iter()
                .any(|a| self.stats.get(a).is_some_and(|s| s.contains_key(m)))
            {
                continue;
            }
            let _ = write!(out, "{m:<14}");
            for a in &self.arms {
                match self.stats.get(a).and_then(|s| s.get(m)) {
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
            if let Some(d) = self.diff.get(m) {
                let ci = d
                    .ci95
                    .map_or_else(|| "?".to_string(), |c| format!("{c:.3}"));
                let mark = if d.significant { " *" } else { "" };
                let _ = write!(out, "{:>26}", format!("{:+.3} ± {ci}{mark}", d.diff));
            }
            out.push('\n');
        }
        if self.arms.len() >= 2 {
            out.push_str("± is the sd per arm, and the 95% interval (Welch) for the difference; * leaves out 0\n");
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
