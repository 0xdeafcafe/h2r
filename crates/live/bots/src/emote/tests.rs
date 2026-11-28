use super::*;
use guest_layout::datum::layout as dl;
use guest_layout::points;
use guest_layout::testing::FakeGuest;

fn kill(attacker: Option<u32>, victim: u32, betrayal: bool, at: Vec3, tick: u32) -> Kill {
    Kill {
        seq: 0,
        tick,
        attacker,
        victim,
        betrayal,
        at: Some(at),
    }
}

const HERE: Vec3 = [0.0; 3];

/// An emoter that always hears (taunt 1, a betrayal is severity 1), with a
/// body at its feet heard at `tick`.
fn armed(tick: u32) -> Emoter {
    let mut e = Emoter::new(1.0, 7);
    e.hear(&[kill(Some(1), 2, true, HERE, tick)]);
    e
}

/// The burst from `tick` on, while nothing interrupts: crouch per frame.
fn play(e: &mut Emoter, from: u32) -> Vec<bool> {
    let mut out = Vec::new();
    for t in from..from + 100 {
        match e.step(HERE, false, false, t) {
            Some(c) => out.push(c),
            None if !out.is_empty() => break,
            None => {}
        }
    }
    out
}

#[test]
fn self_kills_and_betrayals_are_dumb_and_a_fair_kill_is_not() {
    let k = |a, v, b| kill(a, v, b, HERE, 0);
    assert_eq!(
        classify(&k(Some(3), 3, false)),
        Some(Dumb::SelfKill),
        "own grenade, suicide"
    );
    assert_eq!(
        classify(&k(None, 3, false)),
        Some(Dumb::SelfKill),
        "the world: a fall"
    );
    assert_eq!(classify(&k(Some(1), 3, true)), Some(Dumb::Betrayal));
    assert_eq!(classify(&k(Some(1), 3, false)), None);
    assert!(Dumb::Betrayal.severity() > Dumb::SelfKill.severity());
}

#[test]
fn the_log_hands_each_reader_only_what_is_new() {
    let log = KillLog::default();
    let (none, c0) = log.since(0);
    assert!(none.is_empty());
    log.push(kill(None, 1, false, HERE, 5));
    log.push(kill(None, 2, false, HERE, 6));
    let (a, c1) = log.since(c0);
    assert_eq!(
        a.iter().map(|k| (k.seq, k.victim)).collect::<Vec<_>>(),
        [(1, 1), (2, 2)]
    );
    assert!(log.since(c1).0.is_empty());
    for i in 0..100 {
        log.push(kill(None, i, false, HERE, 7));
    }
    let (all, c2) = log.since(0);
    assert_eq!(all.len(), KEPT, "old deaths fall off");
    assert_eq!(c2, 102);
}

#[test]
fn taunts_off_never_emotes_and_the_chance_follows_taunt_and_severity() {
    let mut off = Emoter::new(0.0, 1);
    off.hear(&[kill(Some(1), 2, true, HERE, 0)]);
    assert_eq!(off.step(HERE, false, false, 1), None);

    let rate = |taunt: f32, betrayal: bool| {
        let mut hits = 0;
        for seed in 0..2000u64 {
            let mut e = Emoter::new(taunt, seed);
            let attacker = if betrayal { Some(1) } else { None };
            e.hear(&[kill(attacker, 2, betrayal, HERE, 0)]);
            hits += e.step(HERE, false, false, 1).is_some() as u32;
        }
        hits as f32 / 2000.0
    };
    assert_eq!(rate(1.0, true), 1.0);
    assert!(
        (rate(1.0, false) - 0.8).abs() < 0.05,
        "self-kill: {}",
        rate(1.0, false)
    );
    assert!((rate(0.3, true) - 0.3).abs() < 0.05, "{}", rate(0.3, true));
    // A death with no body, or a fair kill, never.
    let mut e = Emoter::new(1.0, 3);
    e.hear(&[Kill {
        at: None,
        ..kill(None, 2, false, HERE, 0)
    }]);
    e.hear(&[kill(Some(1), 2, false, HERE, 0)]);
    assert_eq!(e.step(HERE, false, false, 1), None);
}

#[test]
fn a_burst_is_three_to_six_presses_at_a_human_rhythm_in_two_seconds() {
    for seed in 0..200 {
        let mut e = Emoter::new(1.0, seed);
        e.hear(&[kill(Some(1), 2, true, HERE, 0)]);
        let frames = play(&mut e, 1);
        let presses = frames.windows(2).filter(|w| !w[0] && w[1]).count() + frames[0] as usize;
        assert!((3..=6).contains(&presses), "seed {seed}: {presses} presses");
        assert!(frames.len() as u32 <= BURST_TICKS);
        // Each press held 2 or 3 ticks of a 4 to 6 tick period (130 to 200 ms).
        let first_release = frames.iter().position(|c| !c).unwrap();
        assert!(
            (2..=3).contains(&first_release),
            "seed {seed}: held {first_release}"
        );
    }
}

#[test]
fn the_body_must_be_near() {
    let mut e = Emoter::new(1.0, 7);
    e.hear(&[kill(Some(1), 2, true, [NEAR + 0.5, 0.0, 0.0], 0)]);
    assert_eq!(e.step(HERE, false, false, 1), None);
    // Walking up to it within the window starts the burst.
    assert_eq!(e.step([0.5, 0.0, 0.0], false, false, 2), Some(true));
}

#[test]
fn no_enemy_in_view_for_a_second_and_a_half() {
    let mut e = armed(0);
    assert_eq!(e.step(HERE, true, false, 1), None, "an enemy in view");
    assert_eq!(e.step(HERE, false, false, 2), None, "just gone");
    assert_eq!(e.step(HERE, false, false, 1 + CLEAR_TICKS - 1), None);
    assert_eq!(e.step(HERE, false, false, 1 + CLEAR_TICKS), Some(true));
}

#[test]
fn an_enemy_stops_the_burst_at_once() {
    let mut e = armed(0);
    assert_eq!(e.step(HERE, false, false, 1), Some(true));
    assert_eq!(e.step(HERE, true, false, 2), None);
    assert!(!e.busy());
    assert_eq!(
        e.step(HERE, false, false, 2 + CLEAR_TICKS),
        None,
        "the body is spent"
    );
}

#[test]
fn no_emote_while_an_objective_is_live() {
    let mut e = armed(0);
    assert_eq!(e.step(HERE, false, true, 1), None);
    assert_eq!(e.step(HERE, false, false, 2), Some(true));
}

#[test]
fn at_most_once_in_thirty_seconds() {
    let mut e = armed(0);
    let end = 1 + play(&mut e, 1).len() as u32;
    e.hear(&[kill(Some(1), 3, true, HERE, end)]);
    assert_eq!(e.step(HERE, false, false, end + 1), None, "resting");
    // A death heard later still has its window when the rest is over.
    let later = end + COOLDOWN_TICKS - 10;
    e.hear(&[kill(Some(1), 4, true, HERE, later)]);
    assert_eq!(e.step(HERE, false, false, end + COOLDOWN_TICKS), Some(true));
}

#[test]
fn a_death_is_forgotten_after_its_window() {
    let mut e = armed(0);
    assert_eq!(e.step(HERE, true, false, 1), None);
    assert_eq!(e.step(HERE, false, false, WINDOW_TICKS + 1), None);
    assert_eq!(e.step(HERE, false, false, WINDOW_TICKS + 2), None);
}

#[test]
fn a_kill_is_read_from_the_kill_functions_arguments() {
    const PH: u32 = 0x0061_0000;
    const PD: u32 = 0x0061_1000;
    const OH: u32 = 0x0062_0000;
    const OD: u32 = 0x0062_1000;
    const BODY: u32 = 0x0063_0000;
    let mut g = FakeGuest::new();
    g.map(points::PLAYERS.base & !0xfff, 0x1000)
        .put_u32(points::PLAYERS.base, PH)
        .map(PH, 0x48)
        .put_u32(PH + dl::ELEMENT_SIZE, 0x21c)
        .put_u32(PH + dl::HIGH_WATER, 3)
        .put_u32(PH + dl::DATA, PD)
        .map(PD, 0x21c * 3);
    g.map(points::OBJECTS.base & !0xfff, 0x1000)
        .put_u32(points::OBJECTS.base, OH)
        .map(OH, 0x48)
        .put_u32(OH + dl::ELEMENT_SIZE, 0xc)
        .put_u32(OH + dl::HIGH_WATER, 1)
        .put_u32(OH + dl::DATA, OD)
        .map(OD, 0xc)
        .map(BODY, 0x200);
    let unit = DatumHandle::new(0xb000, 0);
    g.put_u16(OD, 0xb000).put_u32(OD + 8, BODY);
    for (k, v) in [4.0f32, 5.0, 6.0].into_iter().enumerate() {
        g.put_f32(BODY + 0x64 + 4 * k as u32, v);
    }
    let victim = DatumHandle::new(0xe002, 2);
    g.put_u16(PD + 2 * 0x21c, 0xe002)
        .put_u32(PD + 2 * 0x21c + 0x2c, unit.raw());
    // Player 1 is on the victim's team (both 0), player 0 on team 1.
    g.put_u16(PD + 0x21c, 0xe001);
    g.put_u16(PD, 0xe000).put_u8(PD + 0xc0, 1);
    // A free-for-all game.
    const GAME: u32 = 0x0064_0000;
    g.map(points::GAME_GLOBALS.base & !0xfff, 0x1000)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .map(GAME, 0x400)
        .put_u32(GAME + crate::guest::OPTIONS, crate::guest::MODE_MULTIPLAYER);
    let teammate = DatumHandle::new(0xe001, 1).raw();
    let enemy = DatumHandle::new(0xe000, 0).raw();
    let k = kill_from_guest(&g, teammate, victim.raw())
        .unwrap()
        .unwrap();
    assert_eq!(
        (k.attacker, k.victim, k.betrayal),
        (Some(1), 2, false),
        "free-for-all has no teammates, though both are on team 0"
    );
    assert_eq!(k.at, Some([4.0, 5.0, 6.0]));
    // A team game: a teammate killed is a betrayal, an enemy is not, nor
    // is a suicide.
    g.put_u32(GAME + crate::guest::OPTIONS + crate::guest::VARIANT_FLAGS, 1);
    let k = kill_from_guest(&g, teammate, victim.raw())
        .unwrap()
        .unwrap();
    assert!(k.betrayal);
    let k = kill_from_guest(&g, enemy, victim.raw()).unwrap().unwrap();
    assert_eq!((k.attacker, k.betrayal), (Some(0), false));
    let k = kill_from_guest(&g, victim.raw(), victim.raw())
        .unwrap()
        .unwrap();
    assert!(!k.betrayal);
    let world = kill_from_guest(&g, u32::MAX, victim.raw())
        .unwrap()
        .unwrap();
    assert_eq!((world.attacker, world.betrayal), (None, false));
    assert_eq!(kill_from_guest(&g, teammate, u32::MAX).unwrap(), None);
}

#[test]
fn the_mind_crouches_over_a_body_and_stands_still() {
    use crate::brain::{Me, Perception};
    let mut m = crate::mind::Mind::new(crate::mind::Skill::Normal, 9);
    m.emote = Emoter::new(1.0, 5);
    m.emote.hear(&[kill(Some(1), 2, true, [1.0, 0.0, 0.0], 0)]);
    let p = Perception {
        observations: Default::default(),
        me: Some(Me {
            position: HERE,
            aim: [1.0, 0.0, 0.0],
            team: 0,
            ..Me::default()
        }),
        others: vec![],
        teams: false,
        tick: 1,
        pickups: vec![],
        objective: None,
        goals: Default::default(),
    };
    let pad = m.step(&p, None);
    assert!(pad.buttons.contains(crate::mind::CROUCH));
    assert_eq!(
        (pad.left_stick, pad.right_stick, pad.right_trigger),
        ((0.0, 0.0), (0.0, 0.0), 0.0)
    );
}

#[test]
fn temperament_showboating_raises_own_kill_taunts_behind_the_same_gates() {
    let k = kill(Some(1), 2, false, HERE, 0);
    let count = |taunt, weight| {
        (0..512)
            .filter(|&seed| {
                let mut e = Emoter::new(taunt, seed);
                e.hear_with_style(&[k], Some(1), weight);
                assert_eq!(e.step(HERE, false, true, 1), None);
                e.step(HERE, false, false, 2).is_some()
            })
            .count()
    };
    assert_eq!(count(0.0, 2.0), 0);
    assert!(count(0.5, 1.8) > count(0.5, 0.4) * 2);

    let mut e = armed(0);
    assert_eq!(e.step(HERE, false, false, 1), Some(true));
    e.taunt = 0.0;
    assert_eq!(e.step(HERE, false, false, 2), None);
    assert!(!e.busy());
}
