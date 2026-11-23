use agent_proto::superbounce::{VerifyParams, VerifyPoint, VerifyTrial};
use superbounce::{Kind, Spot, Status, Vec3, verified};

use super::plan::{self, Prime, TrialKind};
use super::summary;
use super::trial::{self, Act, PRIMED, Read, Run};
use super::{Seat, parse_seat, stick_for};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn crouch_spot(stand: Vec3, out: [f32; 2], score: f32) -> Spot {
    Spot {
        kind: Kind::Crouch,
        edge: [stand, stand],
        stand,
        out,
        score,
        ..Spot::default()
    }
}

fn strip(stand: Vec3, score: f32) -> Spot {
    Spot {
        kind: Kind::Bounce,
        // Along X, the drift +Y.
        edge: [
            [stand[0] - 0.5, stand[1], stand[2]],
            [stand[0] + 0.5, stand[1], stand[2]],
        ],
        stand,
        drift: [0.0, 1.0],
        fall: 1.0,
        launch: 11.0,
        need: 4.0,
        score,
        ..Spot::default()
    }
}

fn ask(kind: &str) -> VerifyParams {
    VerifyParams {
        kind: kind.into(),
        ..VerifyParams::default()
    }
}

/// The seat is read at the origin, the frame of every spot's `stand` and
/// of the teleport, not at the bounding sphere's centre about 0.4 over it.
#[test]
fn the_seat_is_read_at_the_origin() {
    use guest_layout::Object;
    assert_eq!(super::OBJECT_POSITION, 0x64);
    assert_eq!(super::OBJECT_POSITION, Object::POSITION.offset);
    assert_ne!(super::OBJECT_POSITION, Object::BOUND_CENTER.offset);
}

// ---- planning ----

#[test]
fn crouch_trials_start_outside_the_lip_on_the_approach_and_face_in() {
    let s = crouch_spot([10.0, 0.0, 1.0], [1.0, 0.0], 0.8);
    let p = plan::plan(&ask("crouch"), &[s]).unwrap();
    // Three turns by two distances by one offset.
    assert_eq!(p.trials.len(), 6);
    let straight = p.trials[0];
    // The way out is +X, so the way in is -X and the start is on +X: out
    // in the standing room, past the lip.
    assert!(close(straight.way_in[0], -1.0) && close(straight.way_in[1], 0.0));
    assert!(close(straight.start[0], 10.3) && close(straight.start[1], 0.0));
    // At stand's height: the game's placement lifts the origin off the
    // floor itself (plan::PLACE_LIFT).
    assert!(close(straight.start[2], 1.0), "{:?}", straight.start);
    assert!(
        close(straight.yaw, std::f32::consts::PI) || close(straight.yaw, -std::f32::consts::PI)
    );
    // It aims just past stand, along the way in.
    assert!(straight.point[0] < 10.0);
    assert_eq!(straight.way_out, [1.0, 0.0]);
    assert_eq!(straight.distance, Some(0.3));
    let far = p.trials[1];
    assert!(close(far.start[0], 10.6));
    // A turned approach starts off the line, still outside.
    let turned = p.trials[2];
    assert!(turned.start[0] > 10.0 && turned.start[1].abs() > 0.05);
    assert!(close(turned.way_in[0].hypot(turned.way_in[1]), 1.0));
}

#[test]
fn the_scan_s_spots_are_counted_best_first_and_picked_by_index() {
    let a = crouch_spot([0.0, 0.0, 0.0], [1.0, 0.0], 0.2);
    let b = crouch_spot([5.0, 0.0, 0.0], [1.0, 0.0], 0.9);
    let c = strip([9.0, 0.0, 0.0], 0.5);
    let scanned = [a, b, c];
    let listed = plan::candidates(&scanned, Kind::Crouch);
    assert_eq!(listed[0].stand, b.stand);
    let p = plan::plan(
        &VerifyParams {
            spots: Some(vec![1]),
            turns: Some(vec![0.0]),
            distances: Some(vec![0.3]),
            ..ask("crouch")
        },
        &scanned,
    )
    .unwrap();
    assert_eq!(p.targets.len(), 1);
    assert_eq!(p.targets[0].index, Some(1));
    assert_eq!(p.targets[0].spot.stand, a.stand);
    assert_eq!(p.trials.len(), 1);
    let e = plan::plan(
        &VerifyParams {
            spots: Some(vec![2]),
            ..ask("crouch")
        },
        &scanned,
    )
    .unwrap_err();
    assert!(e.contains("no crouch spot 2"), "{e}");
}

#[test]
fn a_batch_leaves_out_failed_spots_unless_asked_and_caps_its_spots() {
    let mut spots: Vec<Spot> = (0..20)
        .map(|i| crouch_spot([i as f32, 0.0, 0.0], [1.0, 0.0], 1.0 - i as f32 * 0.01))
        .collect();
    spots[0].status = Status::Failed;
    let p = plan::plan(&ask("crouch"), &spots).unwrap();
    assert_eq!(p.targets.len(), plan::MAX_SPOTS);
    assert_eq!(p.targets[0].index, Some(1), "the failed best is skipped");
    let p = plan::plan(
        &VerifyParams {
            max_spots: Some(2),
            ..ask("crouch")
        },
        &spots,
    )
    .unwrap();
    assert_eq!(p.targets.len(), 2);
    assert_eq!(p.targets[0].index, Some(0));
}

#[test]
fn drop_trials_cross_the_slot_toward_the_drift_at_each_height() {
    let s = strip([0.0, 0.0, 2.0], 0.5);
    let p = plan::plan(
        &VerifyParams {
            across: Some(vec![0.0, 0.1]),
            heights: Some(vec![2.0]),
            ..ask("drop")
        },
        &[s],
    )
    .unwrap();
    assert_eq!(p.trials.len(), 2);
    let t = p.trials[1];
    // Across is along the drift (+Y), from the midline.
    assert!(close(t.point[0], 0.0) && close(t.point[1], 0.1));
    // The origin falls from 2.0 over stand: asked PLACE_LIFT under it,
    // since the game's placement puts the origin that far over the point.
    assert!(close(t.start[2] + plan::PLACE_LIFT, 4.0), "{:?}", t.start);
    assert!(close(t.point[2], 2.0), "{:?}", t.point);
    assert_eq!(t.offset, [0.1, 0.0]);
    assert_eq!(t.height, Some(2.0));
    assert!(close(t.yaw, std::f32::consts::FRAC_PI_2));
    assert_eq!(p.config.prime, Prime::Set);
    assert!(p.config.save);
}

#[test]
fn explicit_points_make_their_own_spots_and_are_not_saved() {
    let p = plan::plan(
        &VerifyParams {
            points: Some(vec![VerifyPoint {
                stand: [1.0, 2.0, 3.0],
                ..VerifyPoint::default()
            }]),
            heights: Some(vec![1.0]),
            across: Some(vec![0.0]),
            ..ask("drop")
        },
        &[],
    )
    .unwrap();
    assert!(!p.config.save);
    let s = p.targets[0].spot;
    assert_eq!(s.kind, Kind::Bounce);
    assert!(close(s.need, plan::POINT_NEED));
    // The midline runs across the default drift (+Y), through stand.
    assert!(
        close(s.edge[0][1], 2.0) && close((s.edge[1][0] - s.edge[0][0]).abs(), plan::POINT_EDGE)
    );
    assert_eq!(p.targets[0].label(0), "#p0");
    let both = VerifyParams {
        spots: Some(vec![0]),
        points: Some(vec![VerifyPoint::default()]),
        ..ask("crouch")
    };
    assert!(
        plan::plan(&both, &[])
            .unwrap_err()
            .contains("one or the other")
    );
}

#[test]
fn routes_go_up_from_the_strip_and_repeat() {
    let s = strip([0.0, 0.0, 1.0], 0.5);
    let p = plan::plan(
        &VerifyParams {
            repeat: Some(2),
            ..ask("route")
        },
        &[s],
    )
    .unwrap();
    assert_eq!(p.trials.len(), 2);
    assert!(close(p.trials[0].start[2], 1.0 + plan::ROUTE_RISE));
    assert!(!p.config.save, "a route writes no verdicts");
    assert_eq!(p.config.mode, bots::bounce_route::Mode::Fun);
}

#[test]
fn bad_params_are_named() {
    let s = crouch_spot([0.0; 3], [1.0, 0.0], 0.5);
    let cases: [(VerifyParams, &str); 6] = [
        (ask("hop"), "crouch, drop or route"),
        (
            VerifyParams {
                hold_ticks: Some(0),
                ..ask("crouch")
            },
            "hold_ticks",
        ),
        (
            VerifyParams {
                turns: Some(vec![120.0]),
                ..ask("crouch")
            },
            "turns",
        ),
        (
            VerifyParams {
                prime: Some("maybe".into()),
                ..ask("crouch")
            },
            "prime",
        ),
        (
            VerifyParams {
                trace: Some(99),
                ..ask("crouch")
            },
            "trace 99",
        ),
        (ask("drop"), "no bounce spots"),
    ];
    for (p, want) in cases {
        let e = plan::plan(&p, &[s]).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let big = VerifyParams {
        turns: Some(vec![0.0; 40]),
        distances: Some(vec![0.3; 20]),
        ..ask("crouch")
    };
    assert!(plan::plan(&big, &[s]).unwrap_err().contains("at most"));
}

#[test]
fn the_estimate_counts_every_trial_at_its_longest() {
    let s = crouch_spot([0.0; 3], [1.0, 0.0], 0.5);
    let p = plan::plan(&ask("crouch"), &[s]).unwrap();
    let each = plan::estimate(&p) / p.trials.len() as u64;
    assert!(each >= u64::from(plan::SETTLE_TICKS + plan::HOLD_TICKS + plan::OUT_TICKS));
}

// ---- seats and the stick ----

#[test]
fn a_seat_is_a_bot_by_name_handle_or_player_else_the_pad() {
    let bots = vec![(7u32, "Clanker".to_string(), Some(2u32))];
    let bot = Seat::Bot {
        handle: 7,
        name: "Clanker".into(),
    };
    assert_eq!(parse_seat("clanker", &bots).unwrap(), bot);
    assert_eq!(parse_seat("7", &bots).unwrap(), bot);
    assert_eq!(parse_seat("player.2", &bots).unwrap(), bot);
    assert_eq!(
        parse_seat("player.0", &bots).unwrap(),
        Seat::Pad { index: 0 }
    );
    assert!(parse_seat("nobody", &bots).is_err());
}

#[test]
fn the_stick_walks_the_world_way_whatever_the_facing() {
    // Facing +Y, walking +Y is straight forward; +X is to the right.
    let up = std::f32::consts::FRAC_PI_2;
    let f = stick_for([0.0, 1.0], up);
    assert!(close(f[0], 0.0) && close(f[1], 1.0));
    let r = stick_for([1.0, 0.0], up);
    assert!(close(r[0], 1.0) && close(r[1], 0.0));
    let big = stick_for([3.0, 4.0], 0.0);
    assert!(close(big[0].hypot(big[1]), 1.0));
}

// ---- the crouch trial ----

fn config(kind: TrialKind) -> plan::Config {
    plan::plan(
        &ask(kind.name()),
        &[crouch_spot([0.0; 3], [1.0, 0.0], 0.5), strip([0.0; 3], 0.5)],
    )
    .unwrap()
    .config
}

/// A body that walks where the pad says, at `speed` units a tick, crouches
/// when the button is down, and is forced crouched under the lip (x under
/// `lip`). Its word turns -3 once it leaves the lip forced, if `primes`.
struct Body {
    pos: Vec3,
    crouch: f32,
    request: bool,
    word: u32,
    forced: bool,
    lip: f32,
    primes: bool,
}

impl Body {
    fn read(&self) -> Read {
        Read {
            pos: Some(self.pos),
            vel: Some([0.0; 3]),
            yaw: Some(0.0),
            airborne: false,
            request: self.request,
            crouch: Some(self.crouch),
            standing_fits: Some(self.pos[0] >= self.lip),
            word: Some(self.word),
        }
    }

    fn apply(&mut self, a: &Act) {
        if let Some(w) = a.word {
            self.word = w;
        }
        if let Some((to, _)) = a.teleport {
            self.pos = to;
        }
        self.pos[0] += a.pad.walk[0] * 0.05;
        self.pos[1] += a.pad.walk[1] * 0.05;
        let under = self.pos[0] < self.lip;
        if a.pad.crouch {
            self.forced = false;
            self.request = true;
        } else if self.request && under {
            self.forced = true;
        } else {
            if self.forced && self.primes {
                self.word = PRIMED;
            }
            self.forced = false;
            self.request = false;
        }
        self.crouch = if self.request {
            (self.crouch + 0.25).min(1.0)
        } else {
            (self.crouch - 0.25).max(0.0)
        };
    }
}

fn run_crouch(primes: bool) -> (VerifyTrial, Vec<Act>) {
    let s = crouch_spot([0.0, 0.0, 0.0], [1.0, 0.0], 0.5);
    let p = plan::plan(
        &VerifyParams {
            turns: Some(vec![0.0]),
            distances: Some(vec![0.3]),
            ..ask("crouch")
        },
        &[s],
    )
    .unwrap();
    let mut run = Run::new(0, p.trials[0], s, &p.config);
    let mut body = Body {
        pos: [5.0, 5.0, 0.0],
        crouch: 0.0,
        request: false,
        // A word left over from before: the settle clears it.
        word: PRIMED,
        forced: false,
        lip: 0.1,
        primes,
    };
    let mut acts = Vec::new();
    for _ in 0..400 {
        let a = run.step(&body.read());
        if a.teleport.is_some() {
            body.apply(&a);
            run.teleported(Ok(()));
        } else {
            body.apply(&a);
        }
        acts.push(a);
        if a.done {
            break;
        }
    }
    (run.outcome(), acts)
}

#[test]
fn a_crouch_trial_crouches_first_then_walks_in_lets_go_and_walks_out() {
    let (out, acts) = run_crouch(true);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.crouched_first, Some(true));
    assert_eq!(out.forced, Some(true));
    assert_eq!(out.under, Some(true));
    assert!(out.closest.unwrap() < trial::REACH);
    assert!(out.primed_at.is_some());
    // The first move is the teleport, outside the lip.
    let (to, _) = acts[0].teleport.unwrap();
    assert!(to[0] > 0.1);
    // The settle cleared the old word.
    assert!(acts.iter().any(|a| a.word == Some(0)));
    // Crouch is pressed standing still before any walk in.
    let first_crouch = acts.iter().position(|a| a.pad.crouch).unwrap();
    assert_eq!(acts[first_crouch].pad.walk, [0.0, 0.0]);
    let first_walk = acts.iter().position(|a| a.pad.walk != [0.0, 0.0]).unwrap();
    assert!(first_walk >= first_crouch + trial::CROUCH_FIRST as usize);
    assert!(acts[first_walk].pad.crouch, "walking in crouched");
    assert!(acts[first_walk].pad.walk[0] < 0.0, "walking in along -X");
    // Then let go, then walk out along +X with the button up.
    let let_go = first_walk
        + acts[first_walk..]
            .iter()
            .position(|a| !a.pad.crouch)
            .unwrap();
    assert!(acts[let_go..].iter().all(|a| !a.pad.crouch));
    assert!(acts[let_go..].iter().any(|a| a.pad.walk[0] > 0.5));
}

#[test]
fn a_crouch_that_is_forced_but_never_primes_is_named_so() {
    let (out, _) = run_crouch(false);
    assert!(!out.ok);
    assert_eq!(out.forced, Some(true));
    assert_eq!(
        summary::reason(TrialKind::Crouch, &out).as_deref(),
        Some("forced_no_word")
    );
}

#[test]
fn a_teleport_that_never_comes_back_ends_the_trial() {
    let s = crouch_spot([0.0; 3], [1.0, 0.0], 0.5);
    let c = config(TrialKind::Crouch);
    let mut run = Run::new(0, plan::crouch_trial(0, &s, 0.0, 0.3, [0.0; 2]), s, &c);
    let r = Read::default();
    assert!(run.step(&r).teleport.is_some());
    assert!(run.waiting());
    let mut ended = false;
    for _ in 0..=plan::TELEPORT_TICKS + 1 {
        if run.step(&r).done {
            ended = true;
            break;
        }
    }
    assert!(ended);
    let out = run.outcome();
    assert_eq!(
        summary::reason(TrialKind::Crouch, &out).as_deref(),
        Some("setup")
    );
}

// ---- the drop trial ----

/// Steps a drop trial through a fall that touches down at `vz_in` and
/// comes out at `vz_out`, at `land`.
fn run_drop(prime: Prime, land: Vec3, vz_in: f32, vz_out: f32) -> (VerifyTrial, Vec<Act>) {
    let s = strip([0.0, 0.0, 0.0], 0.5);
    let mut c = config(TrialKind::Drop);
    c.prime = prime;
    let t = plan::drop_trial(0, &s, 0.0, 0.0, 2.0);
    let mut run = Run::new(0, t, s, &c);
    let mut acts = Vec::new();
    let mut word = 0u32;
    let mut vz = 0.0f32;
    let mut falling = 0;
    let mut pos = [0.0f32; 3];
    for _ in 0..400 {
        let r = Read {
            pos: Some(pos),
            vel: Some([0.0, 0.0, vz]),
            word: Some(word),
            airborne: vz != 0.0,
            ..Read::default()
        };
        let a = run.step(&r);
        if let Some(w) = a.word {
            word = w;
        }
        if a.teleport.is_some() {
            run.teleported(Ok(()));
        }
        if run.phase() == "fall" {
            falling += 1;
            // Falls for 10 ticks, then lands.
            if falling < 10 {
                vz = -vz_in;
                pos = [land[0], land[1], 1.0];
            } else if falling == 10 {
                vz = vz_out;
                pos = [land[0], land[1], 0.0];
            }
        }
        acts.push(a);
        if a.done {
            break;
        }
    }
    (run.outcome(), acts)
}

#[test]
fn a_primed_drop_in_the_slot_that_launches_is_a_bounce() {
    let (out, acts) = run_drop(Prime::Set, [0.0, 0.05, 0.0], 6.0, 11.0);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.landing.as_deref(), Some("bounce"));
    assert!(close(out.speed_in.unwrap(), 6.0));
    assert!(out.vz_out.unwrap() > 8.0);
    // Teleported twice: onto the strip, then up.
    assert_eq!(acts.iter().filter(|a| a.teleport.is_some()).count(), 2);
    assert!(acts.iter().any(|a| a.word == Some(PRIMED)));
    assert_eq!(out.word_before, Some(-3));
}

#[test]
fn a_drop_that_does_not_launch_is_classified_like_the_hud_s() {
    let (out, _) = run_drop(Prime::Set, [0.0, 0.5, 0.0], 6.0, 0.0);
    assert_eq!(out.landing.as_deref(), Some("missed"));
    let (out, _) = run_drop(Prime::Set, [0.0, 0.0, 0.0], 2.0, 0.0);
    assert_eq!(out.landing.as_deref(), Some("too_slow"));
    let (out, acts) = run_drop(Prime::None, [0.0, 0.0, 0.0], 6.0, 0.0);
    assert_eq!(out.landing.as_deref(), Some("not_primed"));
    assert!(
        acts.iter().all(|a| a.word != Some(PRIMED)),
        "the control never primes"
    );
}

// ---- the summary ----

fn crouch_outcome(index: usize, spot: usize, ok: bool, start: Vec3, yaw: f32) -> VerifyTrial {
    VerifyTrial {
        index,
        spot,
        start,
        yaw: Some(yaw),
        distance: Some(0.3),
        ok,
        forced: Some(true),
        closest: Some(0.05),
        primed_tick: ok.then_some(4),
        ..VerifyTrial::default()
    }
}

#[test]
fn spots_are_proven_by_one_success_failed_by_all_failing_and_untried_otherwise() {
    let a = crouch_spot([1.0, 0.0, 0.0], [1.0, 0.0], 0.5);
    let b = crouch_spot([5.0, 0.0, 0.0], [1.0, 0.0], 0.4);
    let c = crouch_spot([9.0, 0.0, 0.0], [1.0, 0.0], 0.3);
    let targets: Vec<plan::Target> = [a, b, c]
        .iter()
        .enumerate()
        .map(|(i, s)| plan::Target {
            index: Some(i),
            spot: *s,
        })
        .collect();
    let outcomes = vec![
        crouch_outcome(0, 0, false, [1.3, 0.0, 0.0], 180.0),
        crouch_outcome(1, 0, true, [1.32, 0.04, 0.0], 175.0),
        crouch_outcome(2, 1, false, [5.3, 0.0, 0.0], 180.0),
        crouch_outcome(3, 1, false, [5.6, 0.0, 0.0], 180.0),
    ];
    let planned = [2, 2, 2];
    let r = summary::results(TrialKind::Crouch, &targets, &planned, &outcomes);
    assert_eq!(r[0].status, "proven");
    assert_eq!(r[0].ok, 1);
    assert_eq!(r[0].best.as_ref().unwrap().trial, 1);
    assert_eq!(r[1].status, "failed");
    assert_eq!(r[1].why.get("forced_no_word"), Some(&2));
    assert_eq!(r[2].status, "untried");
    let lines = summary::lines(TrialKind::Crouch, &targets, &r);
    assert!(
        lines[0].starts_with("crouch #0 (1.00, 0.00, 0.00): primed 1/2, best offset"),
        "{}",
        lines[0]
    );
    assert!(lines[0].contains("yaw 175"), "{}", lines[0]);
    assert!(
        lines[1].contains("(failed)") && lines[1].contains("forced_no_word 2"),
        "{}",
        lines[1]
    );
    // The verified file: the proven spot with where it started and its
    // way in, the failed one, nothing for the untried; and back again.
    let entries = summary::entries(TrialKind::Crouch, &targets, &r, &outcomes);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, Status::Proven);
    assert!(close(entries[0].offset[0], 0.32) && close(entries[0].offset[1], 0.04));
    assert_eq!(entries[0].yaw, Some(175.0));
    assert_eq!(entries[1].status, Status::Failed);
    let back = verified::parse(&verified::to_text(&entries)).unwrap();
    assert_eq!(back.len(), 2);
    let mut spots = vec![a, b, c];
    verified::apply(&mut spots, &back);
    assert_eq!(spots[0].status, Status::Proven);
    assert_eq!(spots[1].status, Status::Failed);
    assert_eq!(spots[2].status, Status::Likely);
    // The bots read the same entry as where to start and which way in.
    let crouch = bots::spots::Crouch::from_spot(&spots[0], verified::entry_for(&back, &spots[0]));
    let (off, yaw) = crouch.start.unwrap();
    assert!(close(off[0], 0.32) && close(yaw.unwrap(), 175.0));
}

#[test]
fn a_drop_s_entry_is_where_it_landed_and_the_height_it_fell() {
    let s = strip([0.0, 0.0, 0.0], 0.5);
    let targets = vec![plan::Target {
        index: Some(0),
        spot: s,
    }];
    let outcomes = vec![
        VerifyTrial {
            index: 0,
            spot: 0,
            ok: false,
            height: Some(1.5),
            vz_out: Some(1.0),
            landing: Some("too_slow".into()),
            ..VerifyTrial::default()
        },
        VerifyTrial {
            index: 1,
            spot: 0,
            ok: true,
            height: Some(2.5),
            offset: [0.05, 0.0],
            vz_out: Some(11.5),
            landed_at: Some([0.0, 0.06, 0.1]),
            landing: Some("bounce".into()),
            ..VerifyTrial::default()
        },
    ];
    let r = summary::results(TrialKind::Drop, &targets, &[2], &outcomes);
    assert_eq!(r[0].status, "proven");
    assert_eq!(r[0].why.get("too_slow"), Some(&1));
    let lines = summary::lines(TrialKind::Drop, &targets, &r);
    assert!(
        lines[0].contains("launched 1/2") && lines[0].contains("launch 11.5"),
        "{}",
        lines[0]
    );
    let e = summary::entries(TrialKind::Drop, &targets, &r, &outcomes);
    assert_eq!(e.len(), 1);
    assert!(close(e[0].offset[1], 0.06));
    assert_eq!(e[0].height, Some(2.5));
    assert!(close(e[0].best, 11.5));
    // Explicit points write nothing.
    let points = vec![plan::Target {
        index: None,
        spot: s,
    }];
    assert!(summary::entries(TrialKind::Drop, &points, &r, &outcomes).is_empty());
}
