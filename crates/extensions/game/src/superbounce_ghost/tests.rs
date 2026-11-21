use std::time::Duration;

use gfx::world_lines::LabelFace;

use super::*;

const SPOT: Vec3 = [5.0, 0.0, 0.0];

fn look() -> Look {
    Look {
        colour: [0.3, 0.6, 1.0],
        face: LabelFace::Hud,
        glyphs: true,
    }
}

/// A bot's practice, tick by tick from tick 1000: it walks along +X, holds
/// crouch on ticks 40..50 of the run, is primed from 50, jumps at 100 (a
/// hop of half a unit), lands on the strip at 130 (the chain's bounce),
/// and climbs out at +12, 0.9 less a tick, to the top at 144. `teleport_at`
/// moves it 10 units at that tick.
fn practice(teleport_at: Option<u32>, crouch: bool) -> (Vec<Sample>, u32) {
    let mut out = Vec::new();
    for k in 0..150u32 {
        let airborne = (100..130).contains(&k) || k > 130;
        let (z, vz) = match k {
            100..130 => {
                let t = (k - 100) as f32 / 30.0;
                (
                    0.5 * (std::f32::consts::PI * t).sin(),
                    3.0 - 0.9 * (k - 100) as f32,
                )
            }
            130.. => (0.2 * (k - 130) as f32, 12.0 - 0.9 * (k - 130) as f32),
            _ => (0.0, 0.0),
        };
        let mut x = k as f32 * 0.05;
        if teleport_at.is_some_and(|t| k >= t) {
            x += 10.0;
        }
        out.push(Sample {
            tick: 1000 + k,
            unit: 7,
            at: [x, 0.0, z],
            yaw: 0.1 * k as f32,
            vz,
            airborne,
            primed: (50..=130).contains(&k),
            request: crouch && (40..50).contains(&k),
        });
    }
    (out, 130)
}

fn record(samples: &[Sample], bounce_at: u32) -> Option<Ghost> {
    let mut t = Track::default();
    let mut out = None;
    for (k, s) in samples.iter().enumerate() {
        let bounce = (k as u32 == bounce_at).then_some(SPOT);
        if let Some(g) = t.step(Some(*s), bounce, Source::Bot) {
            assert!(out.is_none(), "one run per bounce");
            out = Some(g);
        }
    }
    out
}

#[test]
fn a_bounce_is_kept_from_the_priming_crouch_to_the_top() {
    let (s, at) = practice(None, true);
    let g = record(&s, at).expect("a run");
    // From a second before the crouch (40 - 30) to the first tick past the
    // top (the climb from +12 at 0.9 a tick ends at tick 144).
    assert_eq!(g.frames[0].at, s[10].at);
    assert_eq!(g.touchdown, 130 - 10);
    assert_eq!(g.launch, 12.0);
    assert_eq!(g.spot, SPOT);
    assert_eq!(g.source, Source::Bot);
    let last = g.frames.len() - 1 + 10;
    assert!(s[last].vz <= 0.0 && s[last - 1].vz > 0.0, "ends at the top");
    assert!(g.crouch_primed());
    assert_eq!(g.fault(), None);
    let presses = g.presses();
    assert_eq!(
        presses,
        [
            file::Press {
                frame: 30,
                button: Button::Crouch
            },
            file::Press {
                frame: 90,
                button: Button::Jump
            }
        ]
    );
    // The bounce itself is no jump.
    assert!(!g.frames[g.touchdown].jump());
    assert_eq!(g.landing(), s[129].at);
    // It round-trips through the file.
    let back = file::parse(&file::to_text(std::slice::from_ref(&g))).unwrap();
    assert_eq!(back[0].frames.len(), g.frames.len());
}

#[test]
fn a_word_set_for_it_starts_a_second_before_the_prime() {
    let (s, at) = practice(None, false);
    let g = record(&s, at).expect("a run");
    assert_eq!(g.frames[0].at, s[20].at);
    assert!(!g.crouch_primed());
}

#[test]
fn a_teleport_starts_the_track_again() {
    // Between the crouch and the jump: the run starts at the teleport.
    let (s, at) = practice(Some(70), true);
    let g = record(&s, at).expect("a run");
    assert_eq!(g.frames[0].at, s[70].at);
    assert!(!g.crouch_primed());
    // During the climb: nothing is kept.
    let (s, at) = practice(Some(135), true);
    assert_eq!(record(&s, at), None);
    // A new unit (a death) or a gap in the ticks is the same.
    let (mut s, at) = practice(None, true);
    for x in &mut s[60..] {
        x.unit = 8;
    }
    assert_eq!(record(&s, at).unwrap().frames[0].at, s[60].at);
    let (mut s, at) = practice(None, true);
    for x in &mut s[60..] {
        x.tick += 5;
    }
    assert_eq!(record(&s, at).unwrap().frames[0].at, s[60].at);
    // No unit clears it.
    let mut t = Track::default();
    let (s, _) = practice(None, true);
    t.step(Some(s[0]), None, Source::Bot);
    assert_eq!(t.len(), 1);
    t.step(None, None, Source::Bot);
    assert!(t.is_empty());
}

#[test]
fn the_kept_run_proves_its_spot() {
    let (s, at) = practice(None, true);
    let g = record(&s, at).unwrap();
    let e = proof(&g, None);
    assert_eq!(
        (e.kind, e.status, e.trials, e.ok),
        (Kind::Bounce, Status::Proven, 1, 1)
    );
    assert_eq!(e.at, SPOT);
    let landing = g.landing();
    assert!((e.working()[0] - landing[0]).abs() < 1e-5);
    assert_eq!((e.height, e.yaw, e.best), (None, None, 12.0));
    let old = Entry {
        kind: Kind::Bounce,
        status: Status::Likely,
        at: SPOT,
        trials: 4,
        ok: 0,
        best: 14.0,
        ..Entry::default()
    };
    let e = proof(&g, Some(&old));
    assert_eq!((e.trials, e.ok, e.best), (5, 1, 14.0));
}

#[test]
fn the_spot_looked_at_is_in_range_and_near_the_middle() {
    let eye = [0.0, 0.0, 0.62];
    let ahead = [1.0, 0.0, 0.0];
    let points = [
        [10.0, 3.0, 0.62],  // 17 degrees off: not
        [10.0, 0.3, 0.62],  // 1.7 degrees: yes
        [10.0, -0.6, 0.62], // 3.4 degrees: yes, but further off
        [20.0, 0.0, 0.62],  // dead ahead, out of range
    ];
    assert_eq!(looked_at(eye, ahead, &points), Some(1));
    assert_eq!(looked_at(eye, ahead, &points[2..]), Some(0));
    assert_eq!(looked_at(eye, ahead, &[points[0], points[3]]), None);
    // Looking down at a mark on the floor.
    let down = [0.8, 0.0, -0.6];
    assert_eq!(looked_at(eye, down, &[[0.83, 0.0, 0.0]]), Some(0));
    // Close by the mark's own size widens the cone: 0.3 off at 1 unit.
    assert_eq!(looked_at(eye, ahead, &[[1.0, 0.3, 0.62]]), Some(0));
    assert_eq!(looked_at(eye, ahead, &[[1.0, 0.6, 0.62]]), None);
    // No aim, no answer.
    assert_eq!(looked_at(eye, [0.0; 3], &points), None);
    assert_eq!(looked_at(eye, [f32::NAN, 0.0, 0.0], &points), None);
}

fn straight(frames: usize) -> Ghost {
    Ghost {
        spot: SPOT,
        source: Source::Bot,
        launch: 12.0,
        touchdown: frames - 2,
        frames: (0..frames)
            .map(|i| Frame {
                at: [i as f32, 0.0, 0.0],
                yaw: 0.0,
                flags: if i == 3 {
                    file::JUMP | file::AIRBORNE
                } else {
                    0
                },
            })
            .collect(),
    }
}

#[test]
fn playback_runs_on_the_clock_at_any_frame_rate() {
    // One unit a tick: x is the run's tick.
    let g = straight(31);
    // At 30, 60 and 120 frames a second, every present gets the pose of
    // its own time: the same speed, and between ticks interpolated.
    for fps in [30.0f32, 60.0, 120.0] {
        let mut last = -1.0f32;
        let frames = (fps * 1.0) as usize;
        for k in 0..=frames {
            let t = k as f32 / fps;
            let p = pose_at(&g, t).unwrap();
            let want = (t * 30.0).min(30.0);
            assert!(
                (p.at[0] - want).abs() < 1e-3,
                "{fps} fps at {t}: {}",
                p.at[0]
            );
            assert!(p.at[0] > last, "moves every present");
            last = p.at[0];
        }
    }
    // A 120 Hz present between ticks: a quarter of the way.
    let p = pose_at(&g, 1.0 / 120.0).unwrap();
    assert!((p.at[0] - 0.25).abs() < 1e-4 && p.index == 0);
    assert!((p.tick - 0.25).abs() < 1e-4);
    // Half a second is tick 15 whatever the rate.
    assert!((pose_at(&g, 0.5).unwrap().at[0] - 15.0).abs() < 1e-3);
    // Past the end it holds on the last frame, then it is over.
    let hold = pose_at(&g, 1.5).unwrap();
    assert!(hold.done && hold.at[0] == 30.0);
    assert_eq!(pose_at(&g, 1.0 + END_HOLD + 0.05), None);
    // A clock before the start, or not a number, is the start.
    assert_eq!(pose_at(&g, -1.0).unwrap().at[0], 0.0);
    assert_eq!(pose_at(&g, f32::NAN).unwrap().at[0], 0.0);
    // The facing takes the short way round.
    let mut turn = straight(3);
    turn.frames[0].yaw = 3.0;
    turn.frames[1].yaw = -3.0;
    let y = pose_at(&turn, 0.5 / 30.0).unwrap().yaw;
    assert!(
        (y.rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs() < 0.01,
        "{y}"
    );
}

#[test]
fn each_present_draws_the_figure_where_the_run_is_then() {
    let g = Arc::new(straight(31));
    let started = Instant::now();
    let sample = GhostSample {
        ghost: Arc::clone(&g),
        started,
        viewer: Some([0.0, -5.0, 0.62]),
        look: look(),
    };
    let figure_x = |dt: f32| {
        let now = started + Duration::from_secs_f32(dt);
        let mut anchors = Vec::new();
        let shown = sample.visit(&|| now, &mut |l| {
            if l.label.is_none() && !l.dashed && l.edges.len() > RING_SEGMENTS {
                anchors.push(l.anchor[0]);
            }
        });
        assert_eq!(shown, Some(ImageValidity::Image));
        anchors
    };
    // Four presents of one 30 Hz image at 120 frames a second: four poses.
    let xs: Vec<f32> = (0..4)
        .flat_map(|k| figure_x(0.25 + k as f32 / 120.0))
        .collect();
    assert_eq!(xs.len(), 4);
    for (k, x) in xs.iter().enumerate() {
        assert!((x - (7.5 + 0.25 * k as f32)).abs() < 1e-2, "{xs:?}");
    }
    // Over: nothing drawn.
    assert!(figure_x(5.0).is_empty());
}

#[test]
fn the_marks_are_a_trail_a_ring_a_figure_and_the_pressed_buttons() {
    let g = straight(31);
    // Just after the jump at tick 3: its A over where it was pressed.
    let m = marks(&g, 5.0 / 30.0, None, &look());
    assert_eq!(m.len(), 4);
    assert!(m[0].dashed && m[0].edges.len() == 15, "every second tick");
    assert_eq!(m[1].edges.len(), RING_SEGMENTS);
    assert_eq!(m[1].anchor, g.landing());
    assert_eq!(m[2].edges.len(), 27 + 8 + 4 + 1);
    assert!((m[2].anchor[0] - 5.0).abs() < 1e-3);
    assert_eq!(m[3].label.as_deref(), Some("\u{e100}"));
    assert_eq!(m[3].style.label_face, LabelFace::Hud);
    assert!((m[3].anchor[0] - 3.0).abs() < 1e-3);
    // A second after it the glyph has gone.
    assert_eq!(marks(&straight(61), 1.5, None, &look()).len(), 3);
    // Without the HUD font the button is a letter.
    assert_eq!(press_text(Button::Jump, false), "A");
    assert_eq!(press_text(Button::Crouch, true), "\u{e110}");
    // The figure stands as tall as the capsule; crouched, as the crouched one.
    let pose = pose_at(&g, 0.0).unwrap();
    let top = |e: &[[Vec3; 2]]| e.iter().flatten().map(|p| p[2]).fold(0.0f32, f32::max);
    let standing = top(&figure(&pose, None));
    let crouched = top(&figure(
        &Pose {
            crouch: true,
            ..pose
        },
        None,
    ));
    assert!(
        (standing - superbounce::STANDING * 0.98).abs() < 0.01,
        "{standing}"
    );
    assert!(crouched < standing * 0.75);
    // Flat to the viewer: seen from -Y, the figure spreads along X.
    let wide = |viewer| {
        let e = figure(&pose, Some(viewer));
        let xs = e.iter().flatten().map(|p| p[0]);
        xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
    };
    assert!(wide([0.0, -5.0, 0.6]) > 0.3);
    assert!(wide([-5.0, 0.0, 0.6]) < 0.31, "the facing tick only");
}

#[test]
fn the_prompt_offers_the_ghost_and_then_to_stop_it() {
    assert_eq!(prompt(false, false), None);
    assert_eq!(prompt(true, false), Some(GhostPrompt::Show));
    assert_eq!(prompt(false, true), Some(GhostPrompt::Stop));
    assert_eq!(prompt(true, true), Some(GhostPrompt::Stop));
    let show = Some(GhostPrompt::Show);
    assert_eq!(hud_verb_with(None, show), Some(Verb::ShowGhost));
    assert_eq!(
        hud_verb_with(Some(Verb::HoldAndWalkUnder), show),
        Some(Verb::ShowGhost)
    );
    assert_eq!(
        hud_verb_with(Some(Verb::HoldAndWalkUnder), None),
        Some(Verb::HoldAndWalkUnder)
    );
    // The recipe's own steps win.
    assert_eq!(
        hud_verb_with(Some(Verb::LetGoOfCrouch), show),
        Some(Verb::LetGoOfCrouch)
    );
    assert_eq!(
        hud_verb_with(Some(Verb::KeepMoving), Some(GhostPrompt::Stop)),
        Some(Verb::KeepMoving)
    );
    let text = Verb::ShowGhost.text();
    assert!(
        text.contains(BUTTON_GLYPH) && text.ends_with("to show ghost"),
        "{text}"
    );
    assert!(Verb::StopGhost.text().contains(BUTTON_GLYPH));
    // The button's glyph is the action table's d-pad up.
    assert_eq!(BUTTON, 8);
    assert_eq!(BUTTON_GLYPH as u32 - 0xe100, u32::from(BUTTON));
}

#[test]
fn a_press_is_the_button_going_down_once() {
    PRESSED.store(false, Ordering::Relaxed);
    HELD.store(false, Ordering::Relaxed);
    publish_button(Some(true));
    publish_button(Some(true));
    assert!(take_press());
    assert!(!take_press(), "held is not pressed again");
    publish_button(None);
    publish_button(Some(false));
    assert!(!take_press());
    publish_button(Some(true));
    assert!(take_press());
}

#[test]
fn the_ghosts_file_sits_beside_the_verified_file() {
    let name = file_name("Ascension");
    assert!(name.ends_with(".ghosts"), "{name}");
    assert_eq!(
        name.trim_end_matches(".ghosts"),
        bots::spots::verified_file_name("Ascension").trim_end_matches(".verified")
    );
    assert!(no_params(&serde_json::json!(null)).is_ok());
    assert!(no_params(&serde_json::json!({"x": 1})).is_err());
}

#[test]
fn the_agent_lists_a_run_with_its_presses() {
    let (s, at) = practice(None, true);
    let g = record(&s, at).unwrap();
    let r = run_reply(3, &g);
    assert_eq!((r.index, r.source.as_str(), r.ticks), (3, "bot", g.ticks()));
    assert!(r.crouch_primed);
    let buttons: Vec<&str> = r.presses.iter().map(|p| p.button.as_str()).collect();
    assert_eq!(buttons, ["crouch", "jump"]);
    assert_eq!(r.at, [5.0, 0.0, 0.0]);
}
