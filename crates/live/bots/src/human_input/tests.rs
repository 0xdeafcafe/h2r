use super::*;
use crate::{
    Brain,
    brain::{Me, Observations, Perception, Seen},
    decisions::radio::Radio,
    glitches::{self, Key, Trick},
    mind::{Doing, Skill},
    tuning::Tuning,
};
use std::sync::Mutex;

#[test]
fn perception_and_motor_delay_add_once_to_first_sight_reaction() {
    let skill = Skill::Legendary;
    let mut immediate = Brain::new(skill, 11).with_tuning(Tuning {
        perception_ticks: Some(0),
        motor_ticks: Some(0),
        button_sigma_ms: Some(0.0),
        ..Default::default()
    });
    let mut human = Brain::new(skill, 11).with_tuning(Tuning {
        button_sigma_ms: Some(0.0),
        ..Default::default()
    });
    let limits = human.tuning().unwrap().input_limits(skill);
    let (fast_board, slow_board) = (Mutex::new(Radio::default()), Mutex::new(Radio::default()));
    let mut fast_fire = None;
    let mut slow_fire = None;
    let mut slow_sight = None;
    let mut slow_lost = None;
    for tick in 0..30 {
        let p = Perception {
            me: Some(Me::default()),
            tick,
            observations: Observations {
                player: Some((0, 0)),
                ..Default::default()
            },
            others: (tick < 15)
                .then_some(Seen {
                    id: 1,
                    position: [5.0, 0.0, 0.2],
                    team: 1,
                })
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let fast = immediate.play(&p, None, &[], &fast_board);
        let slow = human.play(&p, None, &[], &slow_board);
        assert_eq!(human.play(&p, None, &[], &slow_board), slow);
        if human.mind.doing == Doing::Fight(1) {
            slow_sight.get_or_insert(tick);
        } else if slow_sight.is_some() {
            slow_lost.get_or_insert(tick);
        }
        if fast.right_trigger > 0.0 {
            fast_fire.get_or_insert(tick);
        }
        if slow.right_trigger > 0.0 {
            slow_fire.get_or_insert(tick);
        }
    }
    assert_eq!(slow_sight, Some(limits.perception_ticks));
    assert_eq!(slow_lost, Some(15 + limits.perception_ticks));
    let fast_fire = fast_fire.expect("the first reaction finishes");
    assert!((5..=8).contains(&fast_fire));
    assert_eq!(
        slow_fire,
        Some(fast_fire + limits.perception_ticks + limits.motor_ticks)
    );
}

#[test]
fn death_discards_queued_live_input() {
    let mut pad = Controller::new(1);
    let limits = Tuning {
        motor_ticks: Some(3),
        button_sigma_ms: Some(0.0),
        ..Default::default()
    }
    .input_limits(Skill::Normal);
    let shot = HostPadState {
        right_trigger: 1.0,
        ..Default::default()
    };
    assert_eq!(pad.step(10, shot, limits), HostPadState::default());
    pad.discard_pending();
    for tick in 11..16 {
        assert_eq!(
            pad.step(tick, HostPadState::default(), limits),
            HostPadState::default()
        );
    }
}

#[test]
fn timing_scatter_is_centered_and_scales_with_skill() {
    let (mut last, samples) = (1.0, 40_000);
    for skill in Skill::ALL {
        let sigma = Tuning::default().input_limits(skill).button_sigma_ms;
        let mut rng = Rng::new(6);
        let mut counts = [0; 3];
        for _ in 0..samples {
            counts[(tick_slip(&mut rng, sigma) + 1) as usize] += 1;
        }
        let slips = (counts[0] + counts[2]) as f32 / samples as f32;
        let mean_abs_gaussian = sigma * (2.0 / std::f32::consts::PI).sqrt();
        assert!(
            (slips - mean_abs_gaussian / TICK_MS).abs() < 0.01,
            "{skill:?}: {counts:?}"
        );
        assert!((counts[0] as i32 - counts[2] as i32).abs() < samples / 100);
        assert!(slips < last);
        last = slips;
    }
}

fn assert_stick_limit(previous: (f32, f32), current: (f32, f32), limit: f32) {
    assert!(current.0.hypot(current.1) <= 1.000_001, "{current:?}");
    assert!((current.0 - previous.0).hypot(current.1 - previous.1) <= limit + 1e-6);
}

#[test]
fn controller_possible_pads_over_seeded_brain_runs() {
    // The match input boundary takes only owned observations and produces
    // an Xbox pad. It never gets a mutable guest-memory reference.
    for skill in Skill::ALL {
        for seed in [1, 8, 19, 77] {
            let mut rng = Rng::new(seed);
            let mut brain = Brain::new(skill, seed).with_tuning(Tuning {
                thumb_switch_ticks: Some(2),
                ..Default::default()
            });
            let mut replay = Brain::new(skill, seed).with_tuning(brain.tuning().unwrap());
            brain.mind.glitches = glitches::Glitches::Lots;
            replay.mind.glitches = brain.mind.glitches;
            let limits = brain.tuning().unwrap().input_limits(skill);
            let (board, replay_board) =
                (Mutex::new(Radio::default()), Mutex::new(Radio::default()));
            brain.epoch = 0;
            replay.epoch = 0;
            let mut previous = HostPadState::default();
            let (mut last_face, mut last_look) = (None, None);
            let (mut faces, mut looks) = (0, 0);
            for tick in 0u32..2500 {
                let close = tick % 110 < 45;
                let me = Me {
                    aim: if close {
                        [1.0, 0.0, 0.0]
                    } else {
                        [rng.signed(), rng.signed(), 0.0]
                    },
                    shield: if close { 1.0 } else { rng.unit() },
                    ..Me::default()
                };
                let p = Perception {
                    me: (tick % 217 < 212).then_some(me),
                    tick,
                    observations: Observations {
                        player: Some((0, 0)),
                        ..Default::default()
                    },
                    others: (close || rng.unit() < 0.92)
                        .then(|| Seen {
                            id: 1,
                            position: if close {
                                [0.8, 0.0, 0.2]
                            } else {
                                [3.0 + 2.0 * rng.signed(), rng.signed(), 0.2]
                            },
                            team: 1,
                        })
                        .into_iter()
                        .collect(),
                    ..Default::default()
                };
                let pad = brain.play(&p, None, &[], &board);
                assert_eq!(pad, replay.play(&p, None, &[], &replay_board));
                assert_stick_limit(previous.left_stick, pad.left_stick, limits.stick_step);
                assert_stick_limit(previous.right_stick, pad.right_stick, limits.stick_step);
                assert!((0.0..=1.0).contains(&pad.left_trigger));
                assert!((0.0..=1.0).contains(&pad.right_trigger));
                let face = pad.buttons.bits() & RIGHT_THUMB_BUTTONS;
                let look = pad.right_stick != (0.0, 0.0) || pad.buttons.bits() & RIGHT_CLICK != 0;
                assert!(face.count_ones() <= 1);
                assert!(face == 0 || !look);
                if face != 0 {
                    assert!(last_look.is_none_or(|t| tick - t >= limits.thumb_switch_ticks));
                    last_face = Some(tick);
                    faces += 1;
                }
                if look {
                    assert!(last_face.is_none_or(|t| tick - t >= limits.thumb_switch_ticks));
                    last_look = Some(tick);
                    looks += 1;
                }
                previous = pad;
            }
            assert!(
                faces > 0 && looks > 0,
                "{skill:?} {seed}: {faces} face, {looks} look"
            );
        }
    }
}

fn intent(keys: glitches::Keys) -> HostPadState {
    let mut pad = HostPadState::default();
    pad.buttons.set(HostButton::South, keys.jump);
    pad.buttons.set(HostButton::East, keys.melee);
    pad.buttons.set(HostButton::West, keys.reload);
    pad.buttons.set(HostButton::North, keys.switch);
    pad.buttons.set(HostButton::LeftStick, keys.crouch);
    pad.right_trigger = if keys.fire { 1.0 } else { 0.0 };
    pad
}

fn down(pad: &HostPadState, key: Key) -> bool {
    match key {
        Key::Fire => pad.right_trigger > 0.0,
        Key::Melee => pad.buttons.contains(HostButton::East),
        Key::Reload => pad.buttons.contains(HostButton::West),
        Key::Switch => pad.buttons.contains(HostButton::North),
        Key::Jump => pad.buttons.contains(HostButton::South),
        Key::Crouch => pad.buttons.contains(HostButton::LeftStick),
    }
}

/// Timing windows for the controller tests. Only the sword fly window is
/// traced to a source. The other intervals are estimates for nominal inputs.
fn fits_window(plan: &glitches::Macro, pads: &[HostPadState]) -> bool {
    use Key::*;
    use Trick::*;
    let trick = plan.trick;
    let mut edges = Vec::new();
    let mut previous = HostPadState::default();
    for (tick, pad) in pads.iter().enumerate() {
        for key in [Melee, Reload, Switch, Fire, Jump, Crouch] {
            if down(pad, key) && !down(&previous, key) {
                edges.push((key, tick as u32));
            }
        }
        previous = *pad;
    }
    if trick == SwordFly {
        // The A adds height, so the fly itself only needs Y, X and R.
        edges.retain(|&(key, _)| key != Jump);
    }
    let required = plan
        .beats
        .iter()
        .filter(|beat| trick != SwordFly || beat.key != Jump);
    if edges.len() != required.clone().count() {
        return false;
    }
    let mut prior = (0, 0);
    for (i, ((key, tick), beat)) in edges.into_iter().zip(required).enumerate() {
        if key != beat.key {
            return false;
        }
        if i != 0 {
            let actual = tick - prior.0;
            let nominal = beat.at - prior.1;
            let min = match (trick, key, i) {
                (Bxr | Bxb, Reload, 1) | (Byyr, Switch, 1) => nominal,
                (DoubleShot | QuadShot, Fire, 1 | 6) => nominal,
                _ => nominal.saturating_sub(1).max(1),
            };
            let max = if trick == SwordFly && key == Reload {
                glitches::FLY_WINDOW
            } else {
                nominal + 1
            };
            if !(min..=max).contains(&actual) {
                return false;
            }
        }
        let overlaps = match (trick, key) {
            (Bxr, Fire) => Some(Reload),
            (DoubleShot | QuadShot, Reload) => Some(Fire),
            _ => None,
        };
        if overlaps.is_some_and(|other| !down(&pads[tick as usize], other)) {
            return false;
        }
        prior = (tick, beat.at);
    }
    true
}

#[test]
fn seeded_glitch_timing_rates() {
    const TRIALS: u64 = 4000;
    for (trick, expected) in [
        (Trick::Bxr, [3222, 3429, 3573, 3736]),
        (Trick::Byyr, [2655, 2991, 3222, 3487]),
        (Trick::Bxb, [3189, 3411, 3558, 3728]),
        (Trick::DoubleShot, [3216, 3424, 3569, 3735]),
        (Trick::QuadShot, [1989, 2462, 2791, 3214]),
        (Trick::SwordFly, [2264, 2605, 2919, 3249]),
        (Trick::CrouchJump, [3919, 3960, 3979, 3990]),
    ] {
        let plan = glitches::plan(trick);
        let mut prior = 0;
        for (skill, expected) in Skill::ALL.into_iter().zip(expected) {
            let limits = Tuning::default().input_limits(skill);
            let successes = (1..=TRIALS)
                .filter(|&seed| {
                    let mut pad = Controller::new(seed);
                    let reports = (0..plan.len() + limits.motor_ticks + 3)
                        .map(|tick| pad.step(tick, intent(plan.keys(tick)), limits))
                        .collect::<Vec<_>>();
                    fits_window(&plan, &reports)
                })
                .count();
            println!("{trick:?} {skill:?} {successes}/{TRIALS}");
            assert!(successes > prior && successes < TRIALS as usize);
            assert!(
                successes.abs_diff(expected) <= 8,
                "{trick:?} {skill:?}: {successes}"
            );
            prior = successes;
        }
    }
}

#[test]
fn the_noob_combo_can_release_its_charge_too_soon() {
    use crate::weapons::CHARGE_TICKS;
    const TRIALS: u64 = 4000;
    let mut prior = 0;
    for (skill, expected) in Skill::ALL.into_iter().zip([3384, 3509, 3625, 3748]) {
        let limits = Tuning::default().input_limits(skill);
        let successes = (1..=TRIALS)
            .filter(|&seed| {
                let mut pad = Controller::new(seed);
                let hold = (0..CHARGE_TICKS + limits.motor_ticks + 3)
                    .filter(|&tick| {
                        let report = pad.step(
                            tick,
                            intent(glitches::Keys {
                                fire: tick < CHARGE_TICKS,
                                ..Default::default()
                            }),
                            limits,
                        );
                        report.right_trigger > 0.0
                    })
                    .count();
                hold >= CHARGE_TICKS as usize
            })
            .count();
        println!("Plasma overcharge {skill:?} {successes}/{TRIALS}");
        assert!(successes > prior && successes < TRIALS as usize);
        assert!(successes.abs_diff(expected) <= 8, "{skill:?}: {successes}");
        prior = successes;
    }
}

#[test]
fn the_super_bounce_requests_a_three_tick_jump_after_settling() {
    let spot = glitches::SPOTS[0];
    for skill in Skill::ALL {
        let limits = Tuning::default().input_limits(skill);
        let mut run = glitches::BounceRun::new(spot, 1, 0, 1);
        let mut pad = Controller::new(77);
        let mut requested = None;
        let mut reported = None;
        for tick in 0..15 {
            let input = run.step(spot.stand, tick);
            if input.jump {
                requested.get_or_insert(tick);
            }
            if pad
                .step(
                    tick,
                    intent(glitches::Keys {
                        jump: input.jump,
                        ..Default::default()
                    }),
                    limits,
                )
                .buttons
                .contains(HostButton::South)
            {
                reported.get_or_insert(tick);
            }
        }
        let nominal = requested.unwrap() + limits.motor_ticks;
        assert!((nominal - 1..=nominal + 1).contains(&reported.unwrap()));
    }
}

#[test]
fn temperament_tracking_noise_and_match_pads_obey_human_limits() {
    use crate::emote::Kill;

    assert_eq!(tracking_noise([1.0, -1.0], 0.08), [1.08, -1.08]);
    assert_eq!(tracking_noise([1.0, -1.0], 1.0), [1.10, -1.10]);
    assert_eq!(tracking_noise([1.0, -1.0], -1.0), [1.0, -1.0]);

    for skill in Skill::ALL {
        let mut bot = Brain::new(skill, 7);
        let board = Mutex::new(Radio::default());
        bot.epoch = 0;
        bot.mind.decisions.personality.composure = 0.0;
        bot.mind.decisions.personality.showboating = 1.0;
        let limits = Tuning::default().input_limits(skill);
        let mut previous = HostPadState::default();
        let mut last_face = None;
        let mut last_look = None;
        for tick in 0..140 {
            let died = matches!(tick, 0 | 4 | 8);
            let p = Perception {
                me: (!died).then(|| Me {
                    aim: if tick % 30 < 18 {
                        [1.0, 0.0, 0.0]
                    } else {
                        [0.0, -1.0, 0.0]
                    },
                    ..Me::default()
                }),
                tick,
                observations: Observations {
                    player: Some((0, 0)),
                    ..Default::default()
                },
                others: vec![Seen {
                    id: 1,
                    position: [0.9, 0.2, 0.0],
                    team: 1,
                }],
                ..Default::default()
            };
            let death = Kill {
                seq: u64::from(tick),
                tick,
                victim: 0,
                attacker: Some(1),
                betrayal: false,
                at: None,
            };
            let kills = if died {
                std::slice::from_ref(&death)
            } else {
                &[]
            };
            let pad = bot.play(&p, None, kills, &board);
            if tick == 8 {
                assert_eq!(bot.view().mood.tilt, 1.0);
            }
            assert_stick_limit(previous.left_stick, pad.left_stick, limits.stick_step);
            assert_stick_limit(previous.right_stick, pad.right_stick, limits.stick_step);
            assert!((0.0..=1.0).contains(&pad.left_trigger));
            assert!((0.0..=1.0).contains(&pad.right_trigger));
            let face = pad.buttons.bits() & RIGHT_THUMB_BUTTONS;
            let look = pad.right_stick != (0.0, 0.0) || pad.buttons.bits() & RIGHT_CLICK != 0;
            assert!(face.count_ones() <= 1);
            assert!(face == 0 || !look);
            if face != 0 {
                assert!(last_look.is_none_or(|t| tick - t >= limits.thumb_switch_ticks));
                last_face = Some(tick);
            }
            if look {
                assert!(last_face.is_none_or(|t| tick - t >= limits.thumb_switch_ticks));
                last_look = Some(tick);
            }
            previous = pad;
        }
        assert!(last_look.is_some(), "{skill:?}: the tilted brain turned");
    }
}

#[test]
fn temperament_perception_delay_preserves_kills_across_a_death() {
    use crate::emote::Kill;

    let board = Mutex::new(Radio::default());
    let mut brain = Brain::new(Skill::Normal, 7).with_tuning(Tuning {
        perception_ticks: Some(3),
        ..Default::default()
    });
    brain.epoch = 0;
    let mut p = Perception {
        me: Some(Me::default()),
        tick: 1,
        observations: Observations {
            player: Some((0, 0)),
            ..Default::default()
        },
        ..Default::default()
    };
    let enemy_down = Kill {
        seq: 1,
        tick: 1,
        victim: 1,
        attacker: Some(0),
        betrayal: false,
        at: None,
    };
    brain.play(&p, None, &[enemy_down], &board);
    assert_eq!(brain.view().mood.streak, 0, "the kill is still queued");

    p.me = None;
    p.tick = 2;
    let death = Kill {
        seq: 2,
        tick: 2,
        victim: 0,
        attacker: Some(2),
        ..enemy_down
    };
    brain.play(&p, None, &[death], &board);
    let mood = brain.view().mood;
    assert!(mood.confidence > 0.0, "the buffered kill contributed first");
    assert!(mood.tilt > 0.0, "the death counted");
    assert_eq!(mood.streak, 0);
    assert_eq!(mood.revenge_target, Some(2));
}
