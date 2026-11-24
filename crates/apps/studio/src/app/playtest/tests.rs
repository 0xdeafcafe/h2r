//! The runner over a fake sim (docs/editor/playtest.md, slice PT4): the
//! files written, partial results, a cancel, a stale mark on a new build
//! key and a cache hit; and the measures over invented traces.

use std::path::PathBuf;

use super::fake::{Fake, KILLS};
use super::*;

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-pt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ready(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

fn setup(tag: &str) -> (Temp, Fake, Runner) {
    let t = Temp::new(tag);
    let fake = Fake::new(t.0.clone());
    (t, fake, Runner::new())
}

fn go(r: &mut Runner, f: &Fake, spec: &Spec, names: &[&str], now: f64) -> Result<Started, String> {
    r.start(&mut f.clone(), spec, &ready(names), now)
}

fn pump(r: &mut Runner, f: &Fake, now: f64) {
    r.pump(&mut f.clone(), now);
}

#[test]
fn a_probe_plays_four_seeds_of_the_first_ready_game_type_and_writes_its_files() {
    let (_t, f, mut r) = setup("probe");
    let s = go(
        &mut r,
        &f,
        &Spec::default(),
        &["Slayer", "Capture the Flag"],
        0.0,
    )
    .unwrap();
    assert_eq!(
        (s.run.as_str(), s.matches, s.parallel, s.cached),
        ("r1", 4, 2, false)
    );
    assert_eq!(s.build, "b1");
    assert_eq!(f.w().navs, ["b1"]);
    let b = f.w().batches[0].clone();
    assert_eq!(
        b.jobs
            .iter()
            .map(|j| (j.game_type.as_str(), j.seed, j.swapped))
            .collect::<Vec<_>>(),
        [
            ("slayer", 1, false),
            ("slayer", 2, false),
            ("slayer", 3, false),
            ("slayer", 4, false)
        ]
    );
    // The files are the sim's own: the batch reads and plans every match.
    let m = sim_bench::config::parse_match(&std::fs::read_to_string(&b.match_file).unwrap(), true)
        .unwrap();
    assert_eq!(m.map, "invented");
    assert_eq!(m.detail.as_deref(), Some("spatial"));
    assert_eq!(m.max_ticks, Some(2 * 60 * 30));
    assert_eq!(m.bots.len(), DEFAULT_BOTS as usize);
    let plan = sim_bench::config::load_batch(&b.batch_file).unwrap();
    assert_eq!(plan.runs.len(), 4);
    assert_eq!(r.run(None).unwrap().unwrap().state, State::Running);
}

#[test]
fn standard_and_soak_play_every_ready_game_type_and_soak_swaps_sides() {
    let (_t, f, mut r) = setup("presets");
    let spec = Spec {
        preset: Preset::Standard,
        ..Spec::default()
    };
    let s = go(&mut r, &f, &spec, &["Slayer", "Capture the Flag"], 0.0).unwrap();
    assert_eq!(s.matches, 16, "8 seeds of each");
    let b = f.w().batches[0].clone();
    let m = sim_bench::config::parse_match(&std::fs::read_to_string(&b.match_file).unwrap(), true)
        .unwrap();
    assert_eq!(m.max_ticks, Some(5 * 60 * 30));

    let spec = Spec {
        preset: Preset::Soak,
        seeds: Some(2),
        bots: Some(6),
        ..Spec::default()
    };
    let s = go(&mut r, &f, &spec, &["Slayer"], 1.0).unwrap();
    assert_eq!(s.matches, 4, "2 seeds, two legs each");
    let b = f.w().batches[1].clone();
    assert_eq!(b.jobs.iter().filter(|j| j.swapped).count(), 2);
    let m = sim_bench::config::parse_match(&std::fs::read_to_string(&b.match_file).unwrap(), true)
        .unwrap();
    assert_eq!((m.max_ticks, m.bots.len()), (None, 6), "to the score limit");
    let plan = sim_bench::config::load_batch(&b.batch_file).unwrap();
    assert_eq!(plan.runs.len(), 4);
    assert!(
        r.run(Some("r1")).unwrap().unwrap().state == State::Cancelled,
        "the newer run cancels it"
    );
}

#[test]
fn only_ready_game_types_are_run_and_the_numbers_are_checked() {
    let (_t, f, mut r) = setup("refused");
    let spec = Spec {
        game_types: vec!["ctf".into()],
        ..Spec::default()
    };
    let e = go(&mut r, &f, &spec, &["Slayer"], 0.0).unwrap_err();
    assert!(e.contains("ctf is not ready"), "{e}");
    let spec = Spec {
        game_types: vec!["capture the flag".into(), "CTF".into()],
        ..Spec::default()
    };
    assert_eq!(
        go(&mut r, &f, &spec, &["Slayer", "Capture the Flag"], 0.0)
            .unwrap()
            .matches,
        4,
        "named once"
    );
    assert!(
        go(&mut r, &f, &Spec::default(), &[], 0.0)
            .unwrap_err()
            .contains("no game type is ready")
    );
    assert!(
        go(&mut r, &f, &Spec::default(), &["Race"], 0.0).is_err(),
        "the sim cannot play it"
    );
    for bad in [
        Spec {
            seeds: Some(0),
            ..Spec::default()
        },
        Spec {
            seeds: Some(MAX_SEEDS + 1),
            ..Spec::default()
        },
        Spec {
            minutes: Some(0),
            ..Spec::default()
        },
        Spec {
            bots: Some(1),
            ..Spec::default()
        },
    ] {
        assert!(go(&mut r, &f, &bad, &["Slayer"], 0.0).is_err(), "{bad:?}");
    }
    assert_eq!(f.w().batches.len(), 1, "only the one good call started");
    assert!(Preset::parse("sprint").is_err());
}

#[test]
fn a_failed_build_or_mesh_starts_nothing() {
    let (_t, f, mut r) = setup("build-fails");
    f.w().fail_build = Some("no map open".into());
    let e = go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap_err();
    assert_eq!(e, "build: no map open");
    f.w().fail_build = None;
    f.w().fail_nav = Some("island".into());
    let e = go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap_err();
    assert_eq!(e, "the bots' mesh: island");
    assert!(f.w().batches.is_empty());
    assert!(r.run(None).unwrap().is_none());
}

#[test]
fn partial_results_show_while_the_rest_play() {
    let (_t, f, mut r) = setup("partial");
    go(&mut r, &f, &Spec::default(), &["Slayer"], 10.0).unwrap();
    let jobs = f.jobs();
    // One match ends: one trace's 12 kills are enough for the heat and
    // nothing else.
    f.end(&jobs[0]);
    pump(&mut r, &f, 70.0);
    let run = r.run(None).unwrap().unwrap();
    assert_eq!((run.done(), run.total(), run.state), (1, 4, State::Running));
    // 120 game seconds in 60 wall seconds.
    assert_eq!(run.speedup(), Some(2.0));
    assert!(run.coarse());
    assert_eq!(run.played(), ["slayer"]);
    let m = run.measures("slayer").unwrap();
    assert_eq!(m.traces, 1);
    let heat = m.heat.as_ref().unwrap();
    assert_eq!(
        heat.deaths.iter().map(|b| b.count).sum::<u32>() as usize,
        KILLS
    );
    assert_eq!(m.spawns.as_ref().unwrap_err().have, 0);
    let s = summary_json(&m);
    assert_eq!(s["heat"], json!({"ok": true}));
    assert_eq!(s["spawns"]["ok"], false);
    assert_eq!(s["spawns"]["refused"]["measure"], "spawns");
    assert!(run.measures("ctf").is_none());
    assert_eq!(measure_json(&m, "heat").unwrap()["ok"], true);
    assert!(measure_json(&m, "nope").is_none());

    // The rest end; a match that is not the run's and a repeat are
    // ignored.
    f.end(&jobs[0]);
    f.end(&JobId {
        game_type: "ctf".into(),
        seed: 1,
        swapped: false,
    });
    for j in &jobs[1..] {
        f.end(j);
    }
    pump(&mut r, &f, 130.0);
    let run = r.run(Some("r1")).unwrap().unwrap();
    assert_eq!((run.done(), run.state), (4, State::Done));
    assert_eq!(run.measures("slayer").unwrap().traces, 4);
    assert_eq!(run.game_seconds(), 480.0);
}

#[test]
fn a_failed_match_is_listed_and_a_run_of_failures_fails() {
    let (_t, f, mut r) = setup("failures");
    go(
        &mut r,
        &f,
        &Spec {
            seeds: Some(2),
            ..Spec::default()
        },
        &["Slayer"],
        0.0,
    )
    .unwrap();
    let jobs = f.jobs();
    f.end(&jobs[0]);
    f.fail(&jobs[1], "the guest faulted");
    pump(&mut r, &f, 5.0);
    let run = r.run(None).unwrap().unwrap();
    assert_eq!(run.state, State::Done, "one trace is a result");
    assert_eq!(run.failed().len(), 1);
    assert_eq!(
        (run.failed()[0].0.seed, run.failed()[0].1.as_str()),
        (2, "the guest faulted")
    );

    let spec = Spec {
        seeds: Some(2),
        minutes: Some(3),
        ..Spec::default()
    };
    go(&mut r, &f, &spec, &["Slayer"], 6.0).unwrap();
    for j in f.jobs() {
        f.fail(&j, "no image");
    }
    pump(&mut r, &f, 7.0);
    let run = r.run(None).unwrap().unwrap();
    assert_eq!((run.state, run.done()), (State::Failed, 2));
    assert!(run.speedup().is_none());
    // A run with a failure is not cached: asking again plays again.
    go(
        &mut r,
        &f,
        &Spec {
            seeds: Some(2),
            ..Spec::default()
        },
        &["Slayer"],
        8.0,
    )
    .unwrap();
    assert_eq!(f.w().batches.len(), 3);
}

#[test]
fn a_cancel_stops_the_batch_and_keeps_what_came_back() {
    let (_t, f, mut r) = setup("cancel");
    assert!(!r.cancel(&mut f.clone(), None).unwrap(), "nothing running");
    go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap();
    let jobs = f.jobs();
    f.end(&jobs[0]);
    pump(&mut r, &f, 1.0);
    assert!(r.cancel(&mut f.clone(), None).unwrap());
    assert_eq!(f.w().cancels, 1);
    // Matches that end after it are not folded in.
    f.end(&jobs[1]);
    pump(&mut r, &f, 2.0);
    let run = r.run(None).unwrap().unwrap();
    assert_eq!((run.state, run.done()), (State::Cancelled, 1));
    assert!(run.measures("slayer").is_some(), "the partial result stays");
    assert!(
        !r.cancel(&mut f.clone(), Some("r1")).unwrap(),
        "already over"
    );
    assert!(r.cancel(&mut f.clone(), Some("r9")).is_err());
    assert_eq!(f.w().cancels, 1);
}

#[test]
fn a_new_build_key_marks_the_old_results_stale() {
    let (_t, f, mut r) = setup("stale");
    go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap();
    f.end_all();
    pump(&mut r, &f, 1.0);
    assert!(!r.run(None).unwrap().unwrap().stale);

    f.w().build_key = "b2".into();
    let s = go(&mut r, &f, &Spec::default(), &["Slayer"], 2.0).unwrap();
    assert_eq!(
        (s.run.as_str(), s.build.as_str(), s.cached),
        ("r2", "b2", false)
    );
    let old = r.run(Some("r1")).unwrap().unwrap();
    assert!(old.stale, "it keeps its result, marked");
    assert!(old.measures("slayer").is_some());
    assert!(!r.run(Some("r2")).unwrap().unwrap().stale);
    assert_eq!(f.w().navs, ["b1", "b2"]);

    // Going back to the first build and asking again answers from the
    // cache and clears the mark.
    f.w().build_key = "b1".into();
    let s = go(&mut r, &f, &Spec::default(), &["Slayer"], 3.0).unwrap();
    assert_eq!((s.run.as_str(), s.cached), ("r1", true));
    assert!(!r.run(Some("r1")).unwrap().unwrap().stale);
    assert!(r.run(Some("r2")).unwrap().unwrap().stale);
}

#[test]
fn the_same_build_match_and_seeds_read_the_cache() {
    let (_t, f, mut r) = setup("cache");
    let a = go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap();
    f.end_all();
    pump(&mut r, &f, 1.0);
    let b = go(&mut r, &f, &Spec::default(), &["Slayer", "Oddball"], 2.0).unwrap();
    assert_eq!(
        (b.run.as_str(), b.cached, b.matches),
        (a.run.as_str(), true, 4)
    );
    assert_eq!(f.w().batches.len(), 1, "nothing started");
    assert_eq!(
        f.w().builds,
        2,
        "the build is asked each time and says it is current"
    );

    // Each of the things a result depends on gives a new run.
    for (n, spec) in [
        Spec {
            seeds: Some(3),
            ..Spec::default()
        },
        Spec {
            minutes: Some(3),
            ..Spec::default()
        },
        Spec {
            bots: Some(6),
            ..Spec::default()
        },
        Spec {
            game_types: vec!["oddball".into()],
            ..Spec::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        let s = go(&mut r, &f, &spec, &["Slayer", "Oddball"], 3.0 + n as f64).unwrap();
        assert!(!s.cached, "{spec:?}");
        f.end_all();
        pump(&mut r, &f, 3.5 + n as f64);
    }
    f.w().image = "img2".into();
    assert!(
        !go(&mut r, &f, &Spec::default(), &["Slayer"], 9.0)
            .unwrap()
            .cached,
        "another image"
    );
    f.end_all();
    pump(&mut r, &f, 10.0);
    f.w().bots = "bots2".into();
    assert!(
        !go(&mut r, &f, &Spec::default(), &["Slayer"], 11.0)
            .unwrap()
            .cached,
        "other bots"
    );
}

#[test]
fn a_running_run_is_not_answered_from_the_cache_and_a_new_call_replaces_it() {
    let (_t, f, mut r) = setup("replace");
    go(&mut r, &f, &Spec::default(), &["Slayer"], 0.0).unwrap();
    // The same spec again while it runs starts another (nothing is cached
    // yet) and cancels the first.
    let s = go(&mut r, &f, &Spec::default(), &["Slayer"], 1.0).unwrap();
    assert_eq!((s.run.as_str(), s.cached), ("r2", false));
    assert_eq!(r.run(Some("r1")).unwrap().unwrap().state, State::Cancelled);
    assert_eq!(f.w().cancels, 1);
}

/// The result on show (slice PT5): taken from the run's matches that ended,
/// again only when one more ends, dimmed by a stale mark, cleared with no run.
#[test]
fn the_shown_result_follows_the_run_and_marks_stale() {
    use crate::app::playtest_shown::Shown;
    let (_t, f, mut r) = setup("shown");
    let mut shown = Shown::default();
    assert!(!shown.is_shown());
    go(&mut r, &f, &Spec { seeds: Some(2), ..Spec::default() }, &["Slayer"], 0.0).unwrap();
    shown.show(r.run(None).unwrap().unwrap(), None);
    assert!(shown.is_shown() && shown.measures.is_none() && shown.partial);
    let first = shown.rev;
    // The same run again changes nothing.
    shown.show(r.run(None).unwrap().unwrap(), None);
    assert_eq!(shown.rev, first);
    // A match ends: measures, and a new revision for the viewport.
    f.end(&f.jobs()[0]);
    pump(&mut r, &f, 1.0);
    shown.show(r.run(None).unwrap().unwrap(), None);
    assert!(shown.measures.is_some() && shown.rev > first);
    assert_eq!((shown.game_type.as_str(), shown.stale, shown.coarse), ("slayer", false, true));
    // A newer build marks it stale, keeping the measures.
    let before = shown.rev;
    r.mark_stale("b2");
    shown.show(r.run(None).unwrap().unwrap(), None);
    assert!(shown.stale && shown.measures.is_some() && shown.rev > before);
    shown.clear();
    assert!(!shown.is_shown() && shown.measures.is_none());
}
