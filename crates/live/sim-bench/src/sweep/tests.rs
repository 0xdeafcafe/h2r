use super::*;
use crate::bounce::Launch;

const SWEEP: &str = r#"
out = "results/bounce-sweep"
batch = "superbounce-batch.toml"
[axes]
drift = [0.1, 0.25]
still_ticks = [2, 4, 8]
"#;

fn sweep(text: &str) -> Result<Sweep, String> {
    plan(
        toml::from_str(text).map_err(|e| e.to_string())?,
        Path::new("/s"),
    )
}

#[test]
fn a_sweep_file_reads_and_its_grid_is_every_combination_after_the_base() {
    let s = sweep(SWEEP).unwrap();
    assert_eq!(s.method, Method::Grid);
    assert_eq!(s.out, Path::new("/s/results/bounce-sweep"));
    assert_eq!(s.batch, Path::new("/s/superbounce-batch.toml"));
    // Axes in the schema's order: still_ticks comes before drift.
    let names: Vec<&str> = s.axes.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["still_ticks", "drift"]);
    let g = s.grid();
    assert_eq!(g.len(), 1 + 3 * 2);
    assert_eq!(g[0].id, "c000");
    assert!(
        g[0].bounce.is_empty() && g[0].key.is_empty(),
        "the base: the built-in route"
    );
    assert_eq!(g[1].key, "still_ticks=2 drift=0.1");
    assert_eq!(g[2].key, "still_ticks=2 drift=0.25");
    assert_eq!(g[6].key, "still_ticks=8 drift=0.25");
    assert_eq!(g[6].id, "c006");
    assert_eq!(g[6].bounce.still_ticks, Some(8));
}

#[test]
fn a_bad_sweep_names_every_problem() {
    let e = sweep(
        r#"
out = "x"
batch = "b.toml"
method = "random"
[base]
drift = 3.0
[axes]
wobble = [1.0]
drift = [2.0]
"#,
    )
    .unwrap_err();
    for want in [
        "axes.wobble: not a bounce field",
        "axes.drift: 2 is not 0 to 1",
        "needs samples",
        "base: bounce.drift",
    ] {
        assert!(e.contains(want), "{want:?} missing from:\n{e}");
    }
    let big = sweep(
        r#"
out = "x"
batch = "b.toml"
[axes]
approach = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
drift = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
still_ticks = [1, 2, 3, 4, 5, 6, 7, 8, 9]
"#,
    )
    .unwrap_err();
    assert!(big.contains("a grid of 576 candidates"), "{big}");
    assert!(
        sweep("out = \"x\"\nbatch = \"b\"\n[axes]\n")
            .unwrap_err()
            .contains("at least one")
    );
}

#[test]
fn random_draws_stay_in_each_axis_range_and_repeat_by_seed() {
    let text = format!("method = \"random\"\nsamples = 20\nseed = 9\n{SWEEP}");
    let s = sweep(&text).unwrap();
    let a = s.random();
    assert_eq!(a.len(), 21);
    for c in &a[1..] {
        let st = c.bounce.still_ticks.unwrap();
        assert!((2..=8).contains(&st), "{st}");
        let d = c.bounce.drift.unwrap();
        assert!((0.1..=0.25).contains(&d), "{d}");
    }
    assert_eq!(a, s.random(), "the same seed, the same draws");
    let other = sweep(&format!(
        "method = \"random\"\nsamples = 20\nseed = 10\n{SWEEP}"
    ))
    .unwrap();
    assert_ne!(a, other.random());
}

#[test]
fn a_coordinate_step_moves_one_axis_from_the_best() {
    let text = format!("method = \"coordinate\"\n{SWEEP}");
    let s = sweep(&text).unwrap();
    assert_eq!(s.rounds(), 2);
    let best = BounceParams {
        drift: Some(0.25),
        ..BounceParams::default()
    };
    let step = s.step(&best, 0, 7);
    let keys: Vec<&str> = step.iter().map(|c| c.key.as_str()).collect();
    // still_ticks = 4 is the built-in value: the best plays it already.
    assert_eq!(
        keys,
        ["still_ticks=2 drift=0.25", "still_ticks=8 drift=0.25"]
    );
    assert_eq!(step[0].id, "c007");
    assert!(same_play(
        &BounceParams {
            still_ticks: Some(4),
            ..best.clone()
        },
        &best
    ));
    assert!(!same_play(&BounceParams::default(), &best));
    // The value the best has already is not tried again.
    let step = s.step(&best, 1, 0);
    assert_eq!(step.len(), 1);
    assert_eq!(step[0].bounce.drift, Some(0.1));
    assert!(s.step(&best, 5, 0).is_empty());
}

#[test]
fn a_candidates_batch_is_the_sweeps_batch_with_one_arm_the_candidate() {
    let orig: BatchConfig = toml::from_str(
        r#"
out = "results/x"
match = "superbounce-practice.toml"
maps = ["zanzibar", "ascension"]
seeds = [1, 2]
mode = "separate"
parallel = 3
[a]
name = "base"
params = "tuning/base.toml"
[a.tuning]
skill = "heroic"
[a.tuning.bounce]
drift = 0.2
approach = 0.9
[b]
name = "other"
"#,
    )
    .unwrap();
    let c = Candidate {
        id: "c004".into(),
        key: "drift=0.3".into(),
        bounce: BounceParams {
            drift: Some(0.3),
            ..BounceParams::default()
        },
    };
    let b = candidate_batch(&orig, Path::new("/d"), &c);
    assert_eq!(b.out, Path::new("."));
    assert_eq!(b.match_file, Path::new("/d/superbounce-practice.toml"));
    assert_eq!(
        (b.maps.clone(), b.seeds.clone(), b.parallel),
        (orig.maps.clone(), orig.seeds.clone(), Some(3))
    );
    assert_eq!((b.mode.clone(), b.b.clone()), (None, None));
    assert_eq!(b.a.name.as_deref(), Some("c004"));
    assert_eq!(
        b.a.params.as_deref(),
        Some(Path::new("/d/tuning/base.toml"))
    );
    let t = b.a.tuning.as_ref().unwrap();
    assert_eq!(t.skill.as_deref(), Some("heroic"));
    let bounce = t.bounce.as_ref().unwrap();
    assert_eq!(
        (bounce.drift, bounce.approach),
        (Some(0.3), Some(0.9)),
        "the candidate over the arm's own"
    );
    // It writes as TOML and reads back the same.
    let text = toml::to_string(&b).unwrap();
    let back: BatchConfig = toml::from_str(&text).unwrap();
    assert_eq!(back, b);
}

fn scored(id: &str, attempts: u64, bounced: u64, launch: Option<f64>) -> Scored {
    Scored {
        id: id.into(),
        key: format!("k{id}"),
        stats: Stats {
            attempts,
            bounced_tries: bounced,
            bounce_rate: if attempts > 0 {
                bounced as f64 / attempts as f64
            } else {
                0.0
            },
            launch: launch.map(|m| Launch {
                n: 1,
                best: m,
                mean: m,
            }),
            ..Stats::default()
        },
        ..Scored::default()
    }
}

#[test]
fn candidates_rank_by_bounce_rate_then_launch_and_too_few_tries_are_last() {
    let mut v = vec![
        scored("c000", 40, 10, Some(2.0)),
        scored("c001", 40, 20, Some(1.0)),
        scored("c002", 40, 20, Some(3.0)),
        scored("c003", 4, 4, Some(9.0)),
        scored("c004", 80, 40, Some(3.0)),
    ];
    rank(&mut v, 10);
    let order: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(order, ["c004", "c002", "c001", "c000", "c003"]);
    let ranks: Vec<Option<usize>> = v.iter().map(|s| s.rank).collect();
    assert_eq!(ranks, [Some(1), Some(2), Some(3), Some(4), None]);
    let r = SweepResult::new(Method::Grid, 10, v, vec!["c009".into()]);
    assert_eq!(r.best.as_deref(), Some("c004"));
    let text = serde_json::to_string(&r).unwrap();
    let back: SweepResult = serde_json::from_str(&text).unwrap();
    assert_eq!(back, r);
    let c = csv(&r);
    let mut lines = c.lines();
    let head = lines.next().unwrap();
    assert!(head.starts_with("rank,id,attempts,bounced_tries,bounce_rate"));
    assert!(head.ends_with(",fly_speed"), "{head}");
    assert!(lines.next().unwrap().starts_with("1,c004,80,40,0.5000,"));
    assert_eq!(c.lines().count(), 6);
    let md = markdown(&r, 3);
    assert!(md.contains("| 1 | c004 |"), "{md}");
    assert!(md.contains("No result: c009"));
    assert!(md.contains("Best: c004"));
    assert!(md.contains("[bounce]"));
    let none = SweepResult::new(Method::Grid, 100, vec![scored("c000", 4, 1, None)], vec![]);
    assert_eq!(none.best, None);
    assert!(markdown(&none, 5).contains("nothing is ranked"));
}

#[test]
fn keys_name_the_fields_set_in_schema_order() {
    let b = BounceParams {
        fly_speed: Some(0.5),
        approach: Some(1.0),
        still_ticks: Some(6),
        ..BounceParams::default()
    };
    assert_eq!(key(&b), "approach=1 still_ticks=6 fly_speed=0.5");
    assert_eq!(key(&BounceParams::default()), "");
}

#[test]
fn a_candidate_is_scored_from_its_arm_and_the_coordinate_walk_plays_each_tuning_once() {
    let s = sweep(
        r#"
out = "r"
batch = "b.toml"
method = "coordinate"
[axes]
drift = [0.1, 0.2]
"#,
    )
    .unwrap();
    let base = s.base_candidate();
    let mut cmp = crate::compare::Comparison::default();
    assert_eq!(score(&base, &cmp), None, "no bounce block: no score");
    let mut stats = scored("x", 30, 15, Some(2.5)).stats;
    stats.primes = 20;
    cmp.bounce.insert(
        "c000".into(),
        crate::compare::BounceArm {
            runs: 3,
            totals: stats.clone(),
            by_map: [("ascension".to_string(), stats.clone())].into(),
        },
    );
    let got = score(&base, &cmp).unwrap();
    assert_eq!(
        (got.id.as_str(), got.runs, got.stats.primes),
        ("c000", 3, 20)
    );
    assert_eq!(got.by_map.len(), 1);

    let all = vec![
        scored("c000", 40, 10, None),
        scored("c001", 40, 30, None),
        scored("c002", 5, 5, None),
    ];
    assert_eq!(leader(&all, 10).unwrap().id, "c001");
    assert_eq!(leader(&all[2..], 10), None, "too few tries leads nothing");

    let step = s.step(&s.base, 0, 1);
    assert_eq!(step.len(), 2);
    let played = vec![base.clone(), step[0].clone()];
    let again = fresh(step.clone(), &played, 7);
    assert_eq!(again.len(), 1, "the played tuning is left out");
    assert_eq!(again[0].id, "c007");
    assert_eq!(again[0].bounce, step[1].bounce);
}
