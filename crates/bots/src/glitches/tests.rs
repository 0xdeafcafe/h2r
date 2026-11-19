//! The glitches' presses and timing, when they are allowed, the show-off
//! clock and the super bounce's run.

use super::*;
use crate::arms::{Ammo, Gun, Profile, Trigger};
use crate::weapons::{Combo, ComboIn};

fn rounds(loaded: i16, capacity: i16, reserve: i16) -> Ammo {
    Ammo::Rounds {
        loaded,
        capacity,
        reserve,
        reserve_max: 108,
    }
}
fn gun(trigger: Trigger, zoom: i16, pellets: i16, reach: f32, ammo: Ammo) -> Gun {
    Gun {
        def: 1,
        ammo,
        profile: Profile {
            trigger,
            zoom,
            pellets,
            shot_speed: 400.0,
            cone_range: reach,
            pull_range: reach,
            rate: 6.0,
        },
    }
}
fn br(ammo: Ammo) -> Gun {
    gun(Trigger::Single, 1, 1, 20.0, ammo)
}
fn magnum(ammo: Ammo) -> Gun {
    gun(Trigger::Single, 0, 1, 10.0, ammo)
}
fn smg(ammo: Ammo) -> Gun {
    gun(Trigger::Automatic, 0, 1, 8.0, ammo)
}
fn sword() -> Gun {
    Gun {
        def: 2,
        ammo: Ammo::None,
        profile: Profile {
            trigger: Trigger::Absent,
            cone_range: 2.0,
            ..Profile::default()
        },
    }
}

/// The presses as (key, at, hold), in order.
fn presses(m: &Macro) -> Vec<(Key, u32, u32)> {
    let mut v: Vec<_> = m.beats.iter().map(|b| (b.key, b.at, b.hold)).collect();
    v.sort_by_key(|&(_, at, _)| at);
    v
}

#[test]
fn each_trick_requests_the_nominal_presses() {
    use Key::*;
    // BXR: melee, the reload as the melee lands, fire.
    assert_eq!(
        presses(&plan(Trick::Bxr)),
        [(Melee, 0, 2), (Reload, 5, 4), (Fire, 7, 2)]
    );
    // BYYR: two switches in the reload's place.
    assert_eq!(
        presses(&plan(Trick::Byyr)),
        [(Melee, 0, 2), (Switch, 5, 2), (Switch, 8, 2), (Fire, 10, 2)]
    );
    // BXB: melee, reload, melee.
    assert_eq!(
        presses(&plan(Trick::Bxb)),
        [(Melee, 0, 2), (Reload, 5, 2), (Melee, 7, 2)]
    );
    // RRX: the second pull as the burst's last round leaves, then X.
    assert_eq!(
        presses(&plan(Trick::DoubleShot)),
        [(Fire, 0, 2), (Fire, 4, 4), (Reload, 6, 2)]
    );
    // RRXYYRRX.
    assert_eq!(
        presses(&plan(Trick::QuadShot)),
        [
            (Fire, 0, 2),
            (Fire, 4, 4),
            (Reload, 6, 2),
            (Switch, 9, 2),
            (Switch, 12, 2),
            (Fire, 15, 2),
            (Fire, 19, 4),
            (Reload, 21, 2)
        ]
    );
    // Sword flying: Y and X a tick apart (the thumb's slide), the pull a
    // little slower, A in the lunge for height.
    assert_eq!(
        presses(&plan(Trick::SwordFly)),
        [(Switch, 0, 1), (Reload, 1, 1), (Fire, 3, 2), (Jump, 11, 3)]
    );
    // Crouch jump: A, crouch near the top.
    assert_eq!(
        presses(&plan(Trick::CrouchJump)),
        [(Jump, 0, 3), (Crouch, 7, 10)]
    );
}

#[test]
fn a_macro_says_what_is_down_on_each_tick_and_ends() {
    let m = plan(Trick::Bxr);
    assert_eq!(m.len(), 9);
    let k = |t| m.keys(t);
    assert!(k(0).melee && k(1).melee && !k(2).melee);
    assert!(!k(4).reload && k(5).reload && k(6).reload);
    assert!(k(7).fire && k(8).fire && !k(9).fire);
    // X is held while R goes down; B is alone.
    assert!(k(7).reload && k(8).reload);
    for t in 0..m.len() {
        let k = k(t);
        assert!(!(k.melee && (k.reload || k.fire)), "tick {t}");
    }
    let run = Run::new(m, 100);
    assert_eq!(run.keys(100).map(|k| k.melee), Some(true));
    assert_eq!(run.keys(108).map(|k| k.fire), Some(true));
    assert_eq!(run.keys(109), None, "over");
}

// ---- when ----

fn gate(setting: Glitches) -> Gate {
    Gate {
        setting,
        ..Gate::default()
    }
}
fn moment(held: Gun, other: Option<Gun>, dist: f32) -> Moment {
    Moment {
        dist,
        on_target: true,
        reacted: true,
        held: Some(held),
        other,
    }
}

#[test]
fn off_never_opens_a_trick() {
    let full = br(rounds(36, 36, 72));
    let cases = [
        moment(full, Some(sword()), 8.0),
        moment(full, Some(magnum(rounds(12, 12, 24))), 0.8),
        moment(full, None, 10.0),
    ];
    for m in cases {
        for in_melee in [false, true] {
            let g = Gate {
                in_melee,
                ..gate(Glitches::Off)
            };
            assert_eq!(opening(&g, &m), None);
        }
    }
    assert_eq!(Glitches::default(), Glitches::Off);
    assert_eq!(chance(Glitches::Off, Skill::Legendary), 0.0);
}

#[test]
fn the_trick_fits_the_moment() {
    let lots = gate(Glitches::Lots);
    let melee = Gate {
        in_melee: true,
        ..lots
    };
    let full = br(rounds(36, 36, 72));
    let part = br(rounds(20, 36, 72));
    // Sword in the other hand, the rifle's reticle red past the lunge.
    assert_eq!(
        opening(&lots, &moment(full, Some(sword()), 8.0)),
        Some(Trick::SwordFly)
    );
    // Within the sword's own lunge, or past the rifle's reticle: no.
    assert_ne!(
        opening(&lots, &moment(full, Some(sword()), 2.5)),
        Some(Trick::SwordFly)
    );
    assert_ne!(
        opening(&lots, &moment(full, Some(sword()), 25.0)),
        Some(Trick::SwordFly)
    );
    // Into a group: no.
    let crowded = Gate {
        crowded: true,
        ..lots
    };
    assert_ne!(
        opening(&crowded, &moment(full, Some(sword()), 8.0)),
        Some(Trick::SwordFly)
    );
    // In melee reach: BXR with a full magazine; BYYR with a part one and
    // a second gun; BXB with nothing to fire and nothing to load.
    assert_eq!(opening(&melee, &moment(full, None, 0.8)), Some(Trick::Bxr));
    let pistol = magnum(rounds(12, 12, 24));
    assert_eq!(
        opening(&melee, &moment(part, Some(pistol), 0.8)),
        Some(Trick::Byyr)
    );
    assert_eq!(
        opening(&melee, &moment(part, None, 0.8)),
        None,
        "X would reload"
    );
    let empty = smg(rounds(0, 60, 0));
    assert_eq!(opening(&melee, &moment(empty, None, 0.8)), Some(Trick::Bxb));
    // No reserve: X cannot reload, so it cancels.
    let last = br(rounds(9, 36, 0));
    assert_eq!(opening(&melee, &moment(last, None, 0.8)), Some(Trick::Bxr));
    // At range with the battle rifle: a quad shot with a second gun and
    // two double shots' rounds, else a double shot.
    assert_eq!(
        opening(&lots, &moment(full, Some(pistol), 10.0)),
        Some(Trick::QuadShot)
    );
    assert_eq!(
        opening(&lots, &moment(full, None, 10.0)),
        Some(Trick::DoubleShot)
    );
    assert_eq!(
        opening(&lots, &moment(br(rounds(4, 36, 72)), None, 10.0)),
        None,
        "not two bursts"
    );
    assert_eq!(
        opening(&lots, &moment(br(rounds(36, 36, 0)), None, 10.0)),
        None,
        "the last magazine just shoots twice"
    );
    // A magnum is not a battle rifle.
    assert_eq!(opening(&lots, &moment(pistol, None, 10.0)), None);
}

#[test]
fn never_carrying_never_losing_never_before_reacting() {
    let full = br(rounds(36, 36, 72));
    let melee = Gate {
        in_melee: true,
        ..gate(Glitches::Lots)
    };
    for g in [
        Gate {
            carrying: true,
            ..melee
        },
        Gate {
            losing: true,
            ..melee
        },
    ] {
        assert_eq!(opening(&g, &moment(full, None, 0.8)), None);
        assert_eq!(opening(&g, &moment(full, Some(sword()), 8.0)), None);
    }
    let unready = Moment {
        reacted: false,
        ..moment(full, None, 0.8)
    };
    assert_eq!(opening(&melee, &unready), None);
}

fn fight(tick: u32, glitch: Gate, held: Gun, other: Option<Gun>, dist: f32) -> ComboIn {
    ComboIn {
        tick,
        target: Some(3),
        dist,
        on_target: true,
        reacted: true,
        held: Some(held),
        other,
        firing: false,
        glitch,
    }
}

/// Ticks of a 20 s melee-range fight on which each combo started a trick.
fn tricks_in_a_fight(setting: Glitches, skill: Skill, seed: u64) -> Vec<(u32, Trick)> {
    let mut c = Combo::new(seed);
    let g = Gate {
        in_melee: true,
        ..gate(setting)
    };
    let full = br(rounds(36, 36, 72));
    let mut started = Vec::new();
    let mut was = false;
    for t in 0..600 {
        let out = c.step(&fight(t, g, full, None, 0.8), skill);
        if let (Some(trick), false) = (out.trick, was) {
            started.push((t, trick));
        }
        was = out.trick.is_some();
    }
    started
}

#[test]
fn the_combo_scheduler_plays_bxr_in_a_fight_with_lots_and_never_with_off() {
    for seed in 1..40u64 {
        assert!(tricks_in_a_fight(Glitches::Off, Skill::Legendary, seed).is_empty());
    }
    let lots: usize = (1..40u64)
        .map(|s| tricks_in_a_fight(Glitches::Lots, Skill::Legendary, s).len())
        .sum();
    let some: usize = (1..40u64)
        .map(|s| tricks_in_a_fight(Glitches::Some, Skill::Legendary, s).len())
        .sum();
    assert!(lots > 2 * some, "lots {lots}, some {some}");
    assert!(some > 0);
    // The presses come out in order on the pad: B, X, R.
    let mut c = Combo::new(5);
    let g = Gate {
        in_melee: true,
        ..gate(Glitches::Lots)
    };
    let full = br(rounds(36, 36, 72));
    let mut seq = Vec::new();
    for t in 0..600 {
        let out = c.step(&fight(t, g, full, None, 0.8), Skill::Legendary);
        if out.trick.is_none() {
            if !seq.is_empty() {
                break;
            }
            continue;
        }
        assert!(out.busy);
        // R goes down with X still held: R counts once it is down.
        let key = if out.melee {
            'B'
        } else if out.trigger == Some(true) {
            'R'
        } else if out.reload {
            'X'
        } else {
            continue;
        };
        if seq.last() != Some(&key) {
            seq.push(key);
        }
    }
    assert_eq!(seq, ['B', 'X', 'R']);
}

#[test]
fn a_trick_stops_when_the_target_is_gone_and_waits_before_the_next() {
    let found = (1..200u64).find_map(|seed| {
        let starts = tricks_in_a_fight(Glitches::Lots, Skill::Legendary, seed);
        (starts.len() >= 2).then_some(starts)
    });
    let starts = found.expect("two tricks in some fight");
    let gap = starts[1].0 - starts[0].0;
    assert!(
        gap >= plan(Trick::Bxr).len() + crate::weapons::TRICK_COOLDOWN,
        "{starts:?}"
    );
    let mut c = Combo::new(9);
    let g = Gate {
        in_melee: true,
        ..gate(Glitches::Lots)
    };
    let full = br(rounds(36, 36, 72));
    let t = (0..600)
        .find(|&t| {
            c.step(&fight(t, g, full, None, 0.8), Skill::Legendary)
                .trick
                .is_some()
        })
        .expect("one starts");
    let gone = ComboIn {
        target: None,
        ..fight(t + 1, g, full, None, 0.8)
    };
    assert_eq!(c.step(&gone, Skill::Legendary).trick, None);
    assert!(!c.tricking());
}

// ---- show-offs ----

fn show_offs(setting: Glitches, skill: Skill, free: impl Fn(u32) -> bool) -> u32 {
    let mut s = ShowOff::new(0xabc);
    (0..10 * 60 * 30)
        .filter(|&t| s.due(t, setting, skill, free(t)))
        .count() as u32
}

#[test]
fn off_never_shows_off_and_lots_does_often_when_calm() {
    for skill in Skill::ALL {
        assert_eq!(show_offs(Glitches::Off, skill, |_| true), 0);
    }
    // Ten calm minutes.
    let lots = show_offs(Glitches::Lots, Skill::Legendary, |_| true);
    assert!((25..=80).contains(&lots), "{lots}");
    let some = show_offs(Glitches::Some, Skill::Legendary, |_| true);
    assert!((5..=14).contains(&some), "{some}");
    assert!(show_offs(Glitches::Lots, Skill::Easy, |_| true) < lots);
    // Never while not calm.
    assert_eq!(show_offs(Glitches::Lots, Skill::Legendary, |_| false), 0);
    // Busy every other ten seconds: fewer, never in the busy stretches.
    let mut s = ShowOff::new(0xabc);
    for t in 0..10 * 60 * 30u32 {
        let free = (t / 300) % 2 == 0;
        let due = s.due(t, Glitches::Lots, Skill::Legendary, free);
        assert!(free || !due, "tick {t}");
    }
}

// ---- the super bounce ----

fn spot() -> Bounce {
    SPOTS[0]
}

/// Runs a bounce with `z(try, tick in the air)` as the height in the air,
/// standing on the spot; the outcome and the ticks taken.
fn bounce(tries: u32, z: impl Fn(u32, u32) -> f32) -> (Option<bool>, u32, BounceRun) {
    let s = spot();
    let mut run = BounceRun::new(s, tries, 0, 3);
    let mut at = [s.stand[0], s.stand[1] - 0.3, s.stand[2]];
    let mut jumped: Option<u32> = None;
    for t in 0..2000 {
        let o = run.step(at, t);
        if let Some(done) = o.done {
            return (Some(done), t, run);
        }
        if o.jump && jumped.is_none() {
            jumped = Some(t);
        }
        if let Some(j) = jumped {
            at[2] = z(run.tries, t - j);
            if !run.airborne() {
                jumped = None;
                at[2] = s.stand[2];
            }
        } else if let Some(w) = o.walk {
            // The stick moves it a little each tick.
            at[0] += w[0] * 0.2;
            at[1] += w[1] * 0.2;
        }
    }
    (None, 2000, run)
}

/// A plain jump: up 1.0 and back down over 30 ticks.
fn plain(t: u32) -> f32 {
    let floor = spot().stand[2];
    let x = t as f32 / 30.0;
    floor + (4.0 * x * (1.0 - x)).max(0.0)
}

#[test]
fn the_super_bounce_walks_on_jumps_and_stops_when_thrown_up() {
    let floor = spot().stand[2];
    // The second landing sinks into the seam and is thrown up.
    let (done, _, run) = bounce(8, |try_, t| {
        if try_ == 1 && t > 30 {
            floor - 0.12 + (t - 30) as f32 * 0.4
        } else {
            plain(t)
        }
    });
    assert_eq!(done, Some(true));
    assert_eq!(run.tries, 1, "the first landing was plain");
}

#[test]
fn a_plain_landing_tries_again_from_a_nudged_place_until_the_tries_run_out() {
    let s = spot();
    let (done, _, run) = bounce(3, |_, t| plain(t));
    assert_eq!(done, Some(false));
    assert_eq!(run.tries, 3);
    let off = [run.target[0] - s.stand[0], run.target[1] - s.stand[1]];
    assert!(off[0].abs() <= s.spread && off[1].abs() <= s.spread);
    assert_ne!(run.target, [s.stand[0], s.stand[1]], "nudged");
    // Caught above the floor, still airborne: counts as a landing too.
    let (done, _, _) = bounce(2, |_, t| if t > 26 { floor_plus(0.1) } else { plain(t) });
    assert_eq!(done, Some(false));
}

fn floor_plus(h: f32) -> f32 {
    spot().stand[2] + h
}

#[test]
fn the_bounce_jump_is_a_standing_jump_and_the_drift_is_toward_the_crease() {
    let s = spot();
    let mut run = BounceRun::new(s, 1, 0, 3);
    // On the spot: it stands still, then jumps with the stick let go.
    let mut first_jump = None;
    for t in 0..20 {
        let o = run.step(s.stand, t);
        if o.jump {
            assert_eq!(o.walk, None, "a standing jump");
            first_jump.get_or_insert(t);
        }
    }
    assert_eq!(first_jump, Some(STILL_TICKS));
    // In the air: a light push along the drift (+y on Ascension).
    let o = run.step(floor3(0.5), 21);
    let w = o.walk.expect("drifting");
    assert!(w[1] > 0.0 && w[1] <= DRIFT && w[0].abs() < 1e-6, "{w:?}");
    // It comes in from lower y: the walk ends behind the spot.
    assert!(s.from()[1] < s.stand[1]);
}

fn floor3(h: f32) -> Vec3 {
    let s = spot();
    [s.stand[0], s.stand[1], s.stand[2] + h]
}

#[test]
fn only_measured_spots_are_listed() {
    assert_eq!(bounces("ascension").len(), 1);
    assert_eq!(bounces("Ascension").len(), 1);
    assert!(bounces("lockout").is_empty());
    assert!(bounces("").is_empty());
    for s in SPOTS {
        assert!(s.source.contains("superbounce.md"), "{}", s.name);
        let d = s.drift[0].hypot(s.drift[1]);
        assert!(d < 1e-6 || (d - 1.0).abs() < 1e-3);
    }
    assert_eq!(Glitches::Off.bounce_tries(), 0);
    assert!(Glitches::Lots.bounce_tries() > Glitches::Some.bounce_tries());
}

#[test]
fn the_setting_s_values_round_trip() {
    for g in Glitches::ALL {
        assert_eq!(Glitches::from_index(g.index()), g);
    }
    assert_eq!(Glitches::from_index(9), Glitches::Lots);
}
