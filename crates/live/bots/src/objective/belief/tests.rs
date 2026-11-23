use super::*;
use crate::objective::{Role, Spot, State, plan_with, role_of};

const RED_HOME: Vec3 = [0.0, 0.0, 0.0];
const BLUE_HOME: Vec3 = [100.0, 0.0, 0.0];

fn me(at: Vec3, team: i16) -> Me {
    Me {
        position: at,
        team,
        ..Me::default()
    }
}
fn flag(team: i16, at: Vec3, carrier: Option<u32>) -> Item {
    Item {
        id: 0xe000_0100 + team as u32,
        team,
        position: at,
        carrier,
    }
}
fn spot(kind: u16, team: Option<i16>, at: Vec3) -> Spot {
    Spot {
        kind,
        team,
        position: at,
    }
}
/// CTF on a long map with nothing in view: the bases only.
fn ctf(me: u32, items: Vec<Item>) -> Objective {
    Objective {
        state: Default::default(),
        game: Game::Ctf,
        me,
        items,
        spots: vec![
            spot(kind::CTF_SPAWN, Some(0), RED_HOME),
            spot(kind::CTF_SPAWN, Some(1), BLUE_HOME),
        ],
        hill: None,
    }
}
fn blind(_: Vec3) -> bool {
    false
}
fn eyes(_: Vec3) -> bool {
    true
}
fn teams(items: &[Item]) -> Vec<(i16, Vec3, Option<u32>)> {
    let mut t: Vec<_> = items
        .iter()
        .map(|i| (i.team, i.position, i.carrier))
        .collect();
    t.sort_by_key(|x| x.0);
    t
}

#[test]
fn flags_never_seen_are_at_their_spawns_and_the_attacker_goes_for_theirs() {
    // The round 1 Midship CTF: no flag in view from the spawn, so every
    // bot read "nothing to take" and stood guard at home for 5 minutes.
    let mut b = Belief::default();
    let bot = me([5.0, 0.0, 0.0], 0);
    let (o, outlook) = b
        .know(&ctf(1, vec![]), &bot, 10, Lean::default(), &blind)
        .unwrap();
    assert_eq!(teams(&o.items), [(0, RED_HOME, None), (1, BLUE_HOME, None)]);
    assert_eq!(o.items[1].id, presumed_id(1));
    assert_eq!(outlook.missing, 0);
    let mate = crate::brain::Seen {
        id: 4,
        position: [3.0, 0.0, 0.0],
        team: 0,
    };
    assert_eq!(role_of(1, &[4]), Role::Attack);
    assert_eq!(
        plan_with(&o, &bot, &[mate], true, &outlook),
        Some(Goal::Go(BLUE_HOME))
    );
}

#[test]
fn a_dropped_flag_is_remembered_out_of_view_and_forgotten_when_seen_gone() {
    let mut b = Belief::default();
    let dropped = flag(1, [60.0, 4.0, 0.0], None);
    let bot = me([40.0, 0.0, 0.0], 0);
    b.know(&ctf(1, vec![dropped]), &bot, 10, Lean::default(), &blind);
    // Out of view: still there as far as it knows, not at the spawn.
    let (o, _) = b
        .know(&ctf(1, vec![]), &bot, 40, Lean::default(), &blind)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, dropped.position, None)));
    // Far away, looking at the spot does not count.
    let far = me([90.0, 40.0, 0.0], 0);
    let (o, _) = b
        .know(&ctf(1, vec![]), &far, 50, Lean::default(), &eyes)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, dropped.position, None)));
    // Near and looking: it is gone, so the spawn is the first guess.
    let close = me([55.0, 4.0, 0.0], 0);
    let (o, _) = b
        .know(&ctf(1, vec![]), &close, 60, Lean::default(), &blind)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, dropped.position, None)));
    let (o, _) = b
        .know(&ctf(1, vec![]), &close, 61, Lean::default(), &|p| {
            p != BLUE_HOME
        })
        .unwrap();
    assert!(teams(&o.items).contains(&(1, BLUE_HOME, None)));
    // Remembered only so long.
    let mut b = Belief::default();
    b.know(&ctf(1, vec![dropped]), &bot, 0, Lean::default(), &blind);
    let (o, _) = b
        .know(&ctf(1, vec![]), &bot, FORGET_LOOSE, Lean::default(), &blind)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, BLUE_HOME, None)));
}

#[test]
fn a_spawn_seen_empty_marks_the_flag_missing_until_it_turns_up() {
    let mut b = Belief::default();
    let at_their_base = me([95.0, 0.0, 0.0], 0);
    let (o, outlook) = b
        .know(&ctf(1, vec![]), &at_their_base, 5, Lean::default(), &eyes)
        .unwrap();
    assert!(outlook.missing(1) && !outlook.missing(0));
    assert!(o.items.iter().all(|i| i.team != 1));
    // An attacker with their flag missing plays as in Slayer.
    assert_eq!(plan_with(&o, &at_their_base, &[], true, &outlook), None);
    // Walking away, it stays missing...
    let mid = me([50.0, 0.0, 0.0], 0);
    let (_, outlook) = b
        .know(&ctf(1, vec![]), &mid, 100, Lean::default(), &blind)
        .unwrap();
    assert!(outlook.missing(1));
    // ...until it is seen...
    let seen = flag(1, [70.0, 0.0, 0.0], None);
    let (o, outlook) = b
        .know(&ctf(1, vec![seen]), &mid, 110, Lean::default(), &blind)
        .unwrap();
    assert!(!outlook.missing(1));
    assert_eq!(
        plan_with(&o, &mid, &[], true, &outlook),
        Some(Goal::Go(seen.position))
    );
    // ...or long enough passes for a return or a capture.
    let mut b = Belief::default();
    b.know(&ctf(1, vec![]), &at_their_base, 5, Lean::default(), &eyes);
    let (o, outlook) = b
        .know(
            &ctf(1, vec![]),
            &mid,
            5 + MISSING_TICKS,
            Lean::default(),
            &blind,
        )
        .unwrap();
    assert!(!outlook.missing(1));
    assert!(teams(&o.items).contains(&(1, BLUE_HOME, None)));
}

#[test]
fn our_flag_taken_out_of_sight_sends_the_defender_to_cut_off_the_carrier() {
    let mut b = Belief::default();
    // Bot 9 of red (with 1 and 4) defends.
    let mates: Vec<crate::brain::Seen> = [1, 4]
        .map(|id| crate::brain::Seen {
            id,
            position: [50.0, 0.0, 0.0],
            team: 0,
        })
        .to_vec();
    let bot = me([3.0, 0.0, 0.0], 0);
    let taken = flag(0, [6.0, 0.0, 0.0], Some(7));
    let (o, outlook) = b
        .know(&ctf(9, vec![taken]), &bot, 10, Lean::default(), &blind)
        .unwrap();
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &outlook),
        Some(Goal::Hunt(7, taken.position))
    );
    // Out of view, it is chased to where it was, briefly...
    let (o, outlook) = b
        .know(&ctf(9, vec![]), &bot, 20, Lean::default(), &blind)
        .unwrap();
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &outlook),
        Some(Goal::Hunt(7, taken.position))
    );
    // ...then the trail is cold: our flag is missing, not home, and the
    // defender heads for their base, where the carrier must go.
    let (o, outlook) = b
        .know(
            &ctf(9, vec![]),
            &bot,
            10 + FORGET_CARRIED,
            Lean::default(),
            &blind,
        )
        .unwrap();
    assert!(outlook.missing(0));
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &outlook),
        Some(Goal::Go(BLUE_HOME))
    );
}

#[test]
fn a_teammates_carry_ending_out_of_view_falls_back_on_the_spawn() {
    let mut b = Belief::default();
    let bot = me([30.0, 0.0, 0.0], 0);
    b.know(
        &ctf(1, vec![flag(1, [50.0, 0.0, 0.0], Some(4))]),
        &bot,
        10,
        Lean::default(),
        &blind,
    );
    let (o, outlook) = b
        .know(&ctf(1, vec![]), &bot, 11, Lean::default(), &blind)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, BLUE_HOME, None)));
    assert_eq!(outlook.missing, 0);
}

#[test]
fn an_unreachable_destination_is_given_up_and_another_plate_used() {
    let mut b = Belief::default();
    let near_plate = [60.0, 0.0, 0.0];
    let far_plate = [90.0, 0.0, 0.0];
    let o = Objective {
        state: State::Assault {
            defending: None,
            armed: 0,
            arming: 0,
            planted: 0,
        },
        game: Game::Assault,
        me: 1,
        items: vec![flag(0, [10.0, 0.0, 0.0], Some(1))],
        spots: vec![
            spot(kind::BOMB_RETURN, Some(1), near_plate),
            spot(kind::BOMB_RETURN, Some(1), far_plate),
        ],
        hill: None,
    };
    let bot = me([10.0, 0.0, 0.0], 0);
    let plan = |b: &mut Belief, tick| {
        let (k, outlook) = b.know(&o, &bot, tick, Lean::default(), &blind).unwrap();
        plan_with(&k, &bot, &[], true, &outlook)
    };
    let goal = plan(&mut b, 1);
    assert_eq!(goal, Some(Goal::Carry(near_plate)));
    // The stuck machine repaths, and repaths: given up after the third.
    b.pursue(goal, false, bot.position, 7, 1);
    b.pursue(goal, false, bot.position, 9, 100);
    assert!(!b.unreachable(near_plate, 100));
    b.pursue(goal, false, bot.position, 10, 200);
    assert!(b.unreachable(near_plate, 201));
    assert_eq!(plan(&mut b, 201), Some(Goal::Carry(far_plate)));
    // The only plate left is kept: the carrier never freezes for want of one.
    b.pursue(Some(Goal::Carry(far_plate)), false, bot.position, 10, 202);
    b.pursue(Some(Goal::Carry(far_plate)), false, bot.position, 13, 300);
    assert!(b.unreachable(far_plate, 301));
    assert!(matches!(plan(&mut b, 301), Some(Goal::Carry(p)) if p == near_plate || p == far_plate));
    // And it is tried again later.
    assert!(!b.unreachable(near_plate, 200 + UNREACHABLE_TICKS));
    assert_eq!(
        plan(&mut b, 200 + UNREACHABLE_TICKS),
        Some(Goal::Carry(near_plate))
    );
}

#[test]
fn standing_on_a_flag_it_cannot_take_gives_it_up_for_a_while() {
    let mut b = Belief::default();
    let ledge = [60.0, 0.0, 0.5];
    let theirs = flag(1, ledge, None);
    let bot = me([60.2, 0.0, 0.0], 0);
    let goal = Some(Goal::Go(ledge));
    b.know(&ctf(1, vec![theirs]), &bot, 0, Lean::default(), &blind);
    b.pursue(goal, true, bot.position, 0, 0);
    b.pursue(goal, true, bot.position, 0, STAND_TICKS - 1);
    assert!(!b.unreachable(ledge, STAND_TICKS - 1));
    b.pursue(goal, true, bot.position, 0, STAND_TICKS);
    assert!(b.unreachable(ledge, STAND_TICKS));
    let (o, outlook) = b
        .know(
            &ctf(1, vec![theirs]),
            &bot,
            STAND_TICKS + 1,
            Lean::default(),
            &blind,
        )
        .unwrap();
    assert!(o.items.iter().all(|i| i.position != ledge));
    assert_eq!(plan_with(&o, &bot, &[], true, &outlook), None);
    // An arming carrier standing on the plate is not "stuck".
    let mut b = Belief::default();
    let carry = Some(Goal::Carry(bot.position));
    for t in 0..300 {
        b.pursue(carry, false, bot.position, 0, t);
    }
    assert!(!b.unreachable(bot.position, 300));
}

#[test]
fn other_games_plan_from_what_is_in_view_and_a_new_match_forgets() {
    let mut b = Belief::default();
    let mut o = ctf(1, vec![]);
    o.game = Game::Oddball;
    assert!(
        b.know(&o, &me(RED_HOME, 0), 1, Lean::default(), &blind)
            .is_none()
    );
    let bot = me([40.0, 0.0, 0.0], 0);
    let dropped = flag(1, [60.0, 4.0, 0.0], None);
    b.know(&ctf(1, vec![dropped]), &bot, 500, Lean::default(), &blind);
    let (o, _) = b
        .know(&ctf(1, vec![]), &bot, 3, Lean::default(), &blind)
        .unwrap();
    assert!(teams(&o.items).contains(&(1, BLUE_HOME, None)));
}

#[test]
fn assault_presumes_a_neutral_bomb_at_its_spawn() {
    let mut b = Belief::default();
    let o = Objective {
        state: State::Assault {
            defending: None,
            armed: 0,
            arming: 0,
            planted: 0,
        },
        game: Game::Assault,
        me: 1,
        items: vec![],
        spots: vec![
            spot(kind::BOMB_SPAWN, None, [50.0, 0.0, 0.0]),
            spot(kind::BOMB_RETURN, Some(1), BLUE_HOME),
        ],
        hill: None,
    };
    let bot = me([10.0, 0.0, 0.0], 0);
    let (k, outlook) = b.know(&o, &bot, 1, Lean::default(), &blind).unwrap();
    assert_eq!(teams(&k.items), [(8, [50.0, 0.0, 0.0], None)]);
    assert_eq!(
        plan_with(&k, &bot, &[], true, &outlook),
        Some(Goal::Go([50.0, 0.0, 0.0]))
    );
}
