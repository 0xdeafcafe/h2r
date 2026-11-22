use std::path::Path;

use super::*;
use crate::config::{parse_match, resolve};

fn no_files(_: &Path) -> Result<BotParams, String> {
    Ok(BotParams::default())
}

const MATCH: &str = r#"
map = "ascension"
team_play = true
superbounce = true
seed = 9

[[bot]]
name = "Bouncer"
"#;

fn file(shards: Option<u32>) -> TrainFile {
    TrainFile {
        out: "results/train".into(),
        match_file: "m.toml".into(),
        maps: vec!["ascension".into(), "lockout".into()],
        seed: 7,
        shards,
        budget: None,
    }
}

fn planned(f: TrainFile, m: &str, over: &Overrides) -> Result<Plan, String> {
    plan(
        f,
        parse_match(m, false).unwrap(),
        &no_files,
        Path::new("/t"),
        over,
    )
}

#[test]
fn a_train_table_parses_with_defaults_and_round_trips() {
    let text = "map = \"ascension\"\nteam_play = true\nsuperbounce = true\n\n[train]\nshard = 1\nshards = 4\n\n[[bot]]\nname = \"Bouncer\"\n";
    let c = parse_match(text, false).unwrap();
    let t = c.train.unwrap();
    assert_eq!(
        t,
        TrainConfig {
            shard: 1,
            shards: 4,
            budget: DEFAULT_BUDGET,
            seed: 0
        }
    );
    assert_eq!(
        parse_match(&toml::to_string(&c).unwrap(), false).unwrap(),
        c
    );
    assert!(resolve(c, &no_files).is_ok());
    // An unknown field is refused.
    assert!(parse_match(&text.replace("[train]", "[train]\nbots = 2"), false).is_err());
}

#[test]
fn a_train_table_is_checked_and_every_problem_listed() {
    let ok = TrainConfig::default();
    assert!(ok.problems(true, false).is_empty());
    let e = ok.problems(false, false);
    assert_eq!(e, ["train: needs superbounce = true"]);
    assert!(ok.problems(true, true)[0].contains("drill"));
    let bad = TrainConfig {
        shard: 4,
        shards: 4,
        budget: 0,
        seed: 0,
    };
    let e = bad.problems(true, false).join("\n");
    assert!(e.contains("shard 4 is not under shards 4"), "{e}");
    assert!(e.contains("budget is at least 1"), "{e}");
    let zero = TrainConfig { shards: 0, ..ok };
    assert!(zero.problems(true, false)[0].contains("shards is 1 to"));
    let many = TrainConfig {
        shards: MAX_SHARDS + 1,
        ..ok
    };
    assert!(many.problems(true, false)[0].contains("shards is 1 to"));
}

#[test]
fn a_match_resolves_a_train_table_only_with_the_practice() {
    let text =
        "map = \"ascension\"\nteam_play = true\n[train]\nshards = 2\n[[bot]]\nname = \"B\"\n";
    let e = resolve(parse_match(text, false).unwrap(), &no_files).unwrap_err();
    assert!(e.contains("train: needs superbounce = true"), "{e}");
    let drill = "map = \"ascension\"\nteam_play = true\nsuperbounce = true\n[drill]\nsite = [1.0, 2.0, 3.0]\n[train]\n[[bot]]\nname = \"B\"\n";
    let e = resolve(parse_match(drill, false).unwrap(), &no_files).unwrap_err();
    assert!(e.contains("drill"), "{e}");
}

#[test]
fn train_plans_disjoint_shards() {
    // Every site is in exactly one shard, whatever the shard count.
    for shards in [1u32, 2, 3, 4, 7] {
        for id in (0u64..200).map(|i| i.wrapping_mul(0x9e37_79b9_7f4a_7c15)) {
            let owners: Vec<u32> = (0..shards).filter(|s| owns(id, *s, shards)).collect();
            assert_eq!(owners.len(), 1, "id {id} shards {shards}");
        }
    }
    assert!(!owns(5, 0, 0));
    assert!(owns(5, 1, 2) && !owns(5, 0, 2));
}

#[test]
fn shard_rows_keep_only_the_shards_sites() {
    let rows: Vec<SiteRow> = (0..6)
        .map(|i| SiteRow {
            stand: [i as f32, 0.0, 0.0],
            state: "open".into(),
            ..SiteRow::default()
        })
        .collect();
    let id = |s: [f32; 3]| s[0] as u64;
    let a = shard_rows(&rows, 0, 3, &id);
    let b = shard_rows(&rows, 1, 3, &id);
    let c = shard_rows(&rows, 2, 3, &id);
    assert_eq!(
        [a.len(), b.len(), c.len()],
        [2, 2, 2],
        "six sites, three shards"
    );
    assert_eq!(b[0].stand, [1.0, 0.0, 0.0]);
}

#[test]
fn a_plan_is_a_run_a_map_and_shard() {
    let p = planned(file(Some(3)), MATCH, &Overrides::default()).unwrap();
    assert_eq!(p.runs.len(), 6);
    assert_eq!(p.maps(), ["ascension", "lockout"]);
    let ids: Vec<&str> = p.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "ascension-shard0",
            "ascension-shard1",
            "ascension-shard2",
            "lockout-shard0",
            "lockout-shard1",
            "lockout-shard2"
        ]
    );
    for (r, run) in p.runs.iter().zip(&p.batch.runs) {
        assert_eq!(r.id, run.id);
        assert_eq!(run.map, r.map);
        assert_eq!(run.config.map, r.map);
        assert_eq!(run.config.seed, 7, "the file's seed is the match's");
        assert_eq!(
            run.config.train,
            Some(TrainConfig {
                shard: r.shard,
                shards: 3,
                budget: DEFAULT_BUDGET,
                seed: 7
            })
        );
        assert_eq!(run.config.superbounce, Some(true));
    }
    assert_eq!(p.batch.out, Path::new("/t/results/train"));
    // The shard count is the file's, not the machine's.
    assert_eq!(
        planned(file(None), MATCH, &Overrides::default())
            .unwrap()
            .runs
            .len(),
        2
    );
}

#[test]
fn the_command_line_goes_over_the_file() {
    let over = Overrides {
        maps: vec!["midship".into()],
        budget: Some(33),
    };
    let p = planned(file(Some(2)), MATCH, &over).unwrap();
    assert_eq!(p.maps(), ["midship"]);
    assert_eq!(p.budget, 33);
    assert!(
        p.batch
            .runs
            .iter()
            .all(|r| r.config.train.unwrap().budget == 33)
    );
    let mut f = file(None);
    f.budget = Some(50);
    assert_eq!(planned(f, MATCH, &Overrides::default()).unwrap().budget, 50);
}

#[test]
fn a_plan_refuses_what_a_run_would() {
    let no_practice = MATCH.replace("superbounce = true", "superbounce = false");
    let e = planned(file(Some(2)), &no_practice, &Overrides::default()).unwrap_err();
    assert!(e.contains("train: needs superbounce = true"), "{e}");
    let e = planned(file(Some(0)), MATCH, &Overrides::default()).unwrap_err();
    assert!(e.contains("shards is 1 to"), "{e}");
    let over = Overrides {
        budget: Some(0),
        ..Overrides::default()
    };
    let e = planned(file(None), MATCH, &over).unwrap_err();
    assert!(e.contains("budget is at least 1"), "{e}");
    let no_bots = "map = \"ascension\"\nsuperbounce = true\n";
    assert!(planned(file(None), no_bots, &Overrides::default()).is_err());
}

#[test]
fn the_train_file_reads_and_refuses_unknown_fields() {
    let text = "out = \"r\"\nmatch = \"m.toml\"\nmaps = [\"ascension\"]\nseed = 1\nshards = 4\nbudget = 120\n";
    let f: TrainFile = toml::from_str(text).unwrap();
    assert_eq!((f.shards, f.budget, f.seed), (Some(4), Some(120), 1));
    assert!(toml::from_str::<TrainFile>(&format!("{text}parallel = 4\n")).is_err());
}

#[test]
fn the_example_train_file_loads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/runtime/sim-examples");
    let p = load(&dir.join("superbounce-train.toml"), &Overrides::default()).unwrap();
    assert_eq!(p.maps(), ["ascension"]);
    assert_eq!(p.shards, 4);
    assert_eq!(p.runs.len(), 4);
    assert!(p.batch.runs.iter().all(|r| r.config.train.is_some()));
}

const LEARNED: &str = "\
# superbounce learned v1
bounce learned -10.620 3.880 -3.190 how=runoff fx=-8.330 fy=2.540 fz=-0.760 ax=-10.540 ay=4.090 yaw=0.0 back=1.20 run=1.00 gain=1.20 letgo=0.030 tries=64 hits=19 n=12 ok=8 score=1.214 best=20.90 deaths=1 stage=compass axis=2 shrink=1 moved=0 scattered=0
bounce dead 5.870 6.360 1.060 how=stand ax=5.910 ay=6.360 drift=0.40 still=4 tries=120 hits=0 n=6 ok=0 score=0.412 why=budget stage=compass axis=1 shrink=2 moved=0 scattered=1
bounce blocked 1.000 2.000 3.000 tries=0 hits=0 why=no_path stage=arms axis=0 shrink=0 moved=0 scattered=0
";

#[test]
fn learned_lines_read_as_rows() {
    let rows = parse_rows(LEARNED).unwrap();
    assert_eq!(rows.len(), 3);
    // Sorted by x, y, z.
    assert_eq!(rows[0].stand, [-10.62, 3.88, -3.19]);
    assert_eq!(rows[0].state, "learned");
    assert_eq!(rows[0].how.as_deref(), Some("runoff"));
    assert_eq!(
        (rows[0].tries, rows[0].hits, rows[0].n, rows[0].ok),
        (64, 19, 12, 8)
    );
    assert!((rows[0].rate() - 8.0 / 12.0).abs() < 1e-9);
    assert_eq!(rows[0].best, 20.9);
    assert_eq!(rows[0].deaths, 1);
    assert_eq!(rows[0].knobs["back"], 1.2);
    assert!(!rows[0].knobs.contains_key("tries"));
    assert_eq!(rows[0].cursor["stage"], "compass");
    assert_eq!(rows[1].state, "blocked");
    assert_eq!(rows[1].why.as_deref(), Some("no_path"));
    assert_eq!(rows[2].why.as_deref(), Some("budget"));
    assert_eq!(rows[2].knobs["still"], 4.0);
    assert_eq!(rows[1].rate(), 0.0);
}

#[test]
fn a_bad_line_is_named() {
    let e = parse_rows("# h\nbounce learned 1 2 3 tries=x\n").unwrap_err();
    assert!(e.starts_with("line 2:") && e.contains("tries=x"), "{e}");
    assert!(
        parse_rows("bounce strange 1 2 3\n")
            .unwrap_err()
            .contains("strange")
    );
    assert!(parse_rows("crouch proven 1 2 3\n").is_err());
    assert!(parse_rows("").unwrap().is_empty());
}

#[test]
fn reopen_turns_dead_and_blocked_back_to_open() {
    let (text, n) = reopen(LEARNED);
    assert_eq!(n, 2);
    let rows = parse_rows(&text).unwrap();
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert_eq!(states, ["learned", "open", "open"]);
    // The dead line keeps its evidence, loses its reason, restarts the search.
    let dead = rows.iter().find(|r| r.stand[0] == 5.87).unwrap();
    assert_eq!((dead.tries, dead.n), (120, 6));
    assert_eq!(dead.why, None);
    assert_eq!(dead.cursor["stage"], "arms");
    assert_eq!(dead.cursor["scattered"], "0");
    // The learned line is byte for byte as it was, and a second pass is a no-op.
    assert!(text.contains(LEARNED.lines().nth(1).unwrap()));
    assert_eq!(reopen(&text).1, 0);
    assert_eq!(reopen(&text).0, text);
}

fn fake_report() -> TrainReport {
    let rows = parse_rows(LEARNED).unwrap();
    report(vec![
        ("ascension".into(), rows),
        ("lockout".into(), Vec::new()),
    ])
}

#[test]
fn the_report_counts_states_and_tries() {
    let r = fake_report();
    assert_eq!(r.format, "h2-sim-train/1");
    let m = &r.maps[0];
    assert_eq!((m.sites, m.tries), (3, 184));
    assert_eq!(m.states["learned"], 1);
    assert_eq!(m.states["dead"], 1);
    assert_eq!(m.states["blocked"], 1);
    assert_eq!(
        m.heading(),
        "map ascension: 3 sites: learned 1, dead 1, blocked 1; 184 tries"
    );
    assert_eq!(r.maps[1].heading(), "map lockout: 0 sites; 0 tries");
}

#[test]
fn train_json_has_a_row_a_site_with_knobs_and_cursor() {
    let v = serde_json::to_value(fake_report()).unwrap();
    assert_eq!(v["format"], "h2-sim-train/1");
    let row = &v["maps"][0]["rows"][0];
    assert_eq!(row["state"], "learned");
    assert_eq!(row["how"], "runoff");
    assert_eq!(row["knobs"]["gain"], 1.2);
    assert_eq!(row["cursor"]["shrink"], "1");
    assert_eq!(row["ok"], 8);
    let back: TrainReport = serde_json::from_value(v).unwrap();
    assert_eq!(back, fake_report());
}

#[test]
fn train_md_is_a_table_a_map() {
    let md = markdown(&fake_report());
    let lines: Vec<&str> = md.lines().collect();
    assert_eq!(
        lines[0],
        "map ascension: 3 sites: learned 1, dead 1, blocked 1; 184 tries"
    );
    let head: Vec<&str> = lines[1].split_whitespace().collect();
    assert_eq!(
        head,
        [
            "site", "state", "how", "tries", "hits", "n", "ok", "rate", "score", "best", "deaths",
            "why"
        ]
    );
    assert!(lines[2].starts_with("(-10.62, 3.88, -3.19)"));
    let first: Vec<&str> = lines[2].split_whitespace().collect();
    assert_eq!(
        &first[3..],
        [
            "learned", "runoff", "64", "19", "12", "8", "0.67", "1.214", "20.9", "1"
        ][..]
    );
    // A site with no best launch shows a dash, a dead one its reason last.
    let dead: Vec<&str> = lines[4].split_whitespace().collect();
    assert_eq!(dead.last(), Some(&"budget"));
    assert!(dead.contains(&"-"));
    // Columns line up under their headings.
    assert_eq!(lines[1].find("state"), lines[2].find("learned"));
    assert!(md.contains("\nmap lockout: 0 sites; 0 tries\n"));
    assert_eq!(markdown(&report(Vec::new())), "no sites\n");
}
