//! CTF and Assault as a team plays them: roles by rank and by the name's
//! leanings ([`Lean`]), the bomb's blast, and what is known out of view
//! ([`Outlook`]).

use super::*;

const RED_HOME: Vec3 = [0.0, 0.0, 0.0];
const BLUE_HOME: Vec3 = [100.0, 0.0, 0.0];

fn me(at: Vec3, team: i16) -> Me {
    Me {
        position: at,
        team,
        ..Me::default()
    }
}
fn seen(id: u32, at: Vec3, team: i16) -> Seen {
    Seen {
        id,
        position: at,
        team,
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
fn spot(kind: u16, team: i16, at: Vec3) -> Spot {
    Spot {
        kind,
        team: Some(team),
        position: at,
    }
}
fn ctf(me_id: u32, items: Vec<Item>) -> Objective {
    Objective {
        state: Default::default(),
        game: Game::Ctf,
        me: me_id,
        items,
        spots: vec![
            spot(kind::CTF_SPAWN, 0, RED_HOME),
            spot(kind::CTF_SPAWN, 1, BLUE_HOME),
        ],
        hill: None,
    }
}
fn home_flags() -> Vec<Item> {
    vec![flag(0, RED_HOME, None), flag(1, BLUE_HOME, None)]
}
fn leaning(lean: Lean) -> Outlook {
    Outlook { lean, missing: 0 }
}
const ESCORT: Lean = Lean {
    aggression: 0.4,
    caution: 0.6,
    teamwork: 0.9,
    focus: 0.5,
};
const RUNNER: Lean = Lean {
    aggression: 0.8,
    caution: 0.3,
    teamwork: 0.4,
    focus: 0.8,
};

#[test]
fn the_nearest_attacker_grabs_and_an_escorting_name_covers_it() {
    // Red 1, 2, 4 and 9: 9 defends, 1, 2 and 4 attack. 2 is nearest
    // their flag.
    let team = [
        seen(1, [40.0, 0.0, 0.0], 0),
        seen(2, [70.0, 0.0, 0.0], 0),
        seen(4, [30.0, 0.0, 0.0], 0),
        seen(9, [2.0, 0.0, 0.0], 0),
    ];
    let others = |id: u32| -> Vec<Seen> { team.iter().copied().filter(|s| s.id != id).collect() };
    let goal = |id: u32, lean| {
        let at = team.iter().find(|s| s.id == id).unwrap().position;
        plan_with(
            &ctf(id, home_flags()),
            &me(at, 0),
            &others(id),
            true,
            &leaning(lean), Temper::default(),
        )
    };
    // The nearest goes whatever its name.
    assert_eq!(goal(2, ESCORT), Some(Goal::Go(BLUE_HOME)));
    // A runner races for it too; an escort covers 2.
    assert_eq!(goal(1, RUNNER), Some(Goal::Go(BLUE_HOME)));
    assert_eq!(goal(1, ESCORT), Some(Goal::Escort([70.0, 0.0, 0.0])));
    // The defender is not someone to escort, and keeps home.
    assert!(matches!(goal(9, ESCORT), Some(Goal::Hold(at, _)) if at == RED_HOME));
}

#[test]
fn bold_defenders_patrol_wider_than_careful_ones() {
    let o = ctf(4, home_flags());
    let mate = [seen(1, [50.0, 0.0, 0.0], 0)];
    let radius = |lean| match plan_with(&o, &me([1.0, 0.0, 0.0], 0), &mate, true, &leaning(lean), Temper::default()) {
        Some(Goal::Hold(at, r)) if at == RED_HOME => r,
        g => panic!("{g:?}"),
    };
    assert!(radius(RUNNER) > radius(ESCORT));
    assert_eq!(radius(Lean::default()), DEFEND_RADIUS);
}

#[test]
fn a_focused_attacker_keeps_on_for_their_flag_past_a_nearer_carrier() {
    let mut items = home_flags();
    items[0] = flag(0, [40.0, 0.0, 0.0], Some(7));
    let o = ctf(1, items);
    let mates = [seen(4, [2.0, 0.0, 0.0], 0)];
    // The carrier is 25 away, their flag 35.
    let bot = me([65.0, 0.0, 0.0], 0);
    let neutral = plan_with(&o, &bot, &mates, true, &Outlook::default(), Temper::default());
    assert_eq!(neutral, Some(Goal::Hunt(7, [40.0, 0.0, 0.0])));
    let focused = Lean {
        focus: 0.9,
        ..Lean::default()
    };
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &leaning(focused), Temper::default()),
        Some(Goal::Go(BLUE_HOME))
    );
}

#[test]
fn a_team_minded_attacker_turns_back_further_for_our_dropped_flag() {
    let mut items = home_flags();
    items[0] = flag(0, [20.0, 0.0, 0.0], None);
    let o = ctf(1, items);
    let mates = [seen(4, [2.0, 0.0, 0.0], 0)];
    let bot = me([50.0, 0.0, 0.0], 0);
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &Outlook::default(), Temper::default()),
        Some(Goal::Go(BLUE_HOME))
    );
    assert_eq!(
        plan_with(&o, &bot, &mates, true, &leaning(ESCORT), Temper::default()),
        Some(Goal::Go([20.0, 0.0, 0.0]))
    );
}

fn assault(id: u32, items: Vec<Item>, planted: u16) -> Objective {
    Objective {
        state: State::Assault {
            defending: None,
            armed: planted,
            arming: 0,
            planted,
        },
        game: Game::Assault,
        me: id,
        items,
        spots: vec![
            spot(kind::BOMB_SPAWN, 0, [12.0, 0.0, 0.0]),
            spot(kind::BOMB_RETURN, 0, RED_HOME),
            spot(kind::BOMB_SPAWN, 1, [88.0, 0.0, 0.0]),
            spot(kind::BOMB_RETURN, 1, BLUE_HOME),
        ],
        hill: None,
    }
}

#[test]
fn nobody_stands_in_a_planted_bombs_blast() {
    // Their bomb planted on our plate: our defender (4 of 1 and 4) does
    // not guard the plate it would die on; it covers from the edge.
    let planted = flag(1, RED_HOME, None);
    let o = assault(4, vec![planted], 1 << 1);
    let bot = me([2.0, 0.0, 0.0], 0);
    let g = plan(&o, &bot, &[seen(1, [60.0, 0.0, 0.0], 0)], true);
    assert_eq!(
        g,
        Some(Goal::Hold([BLAST_CLEAR + CLEAR_HOLD, 0.0, 0.0], CLEAR_HOLD))
    );
    // Our own bomb planted on their plate: the planter's side heads home
    // to guard its own, out of the blast.
    let ours = flag(0, BLUE_HOME, None);
    let o = assault(1, vec![ours], 1 << 0);
    let bot = me([97.0, 4.0, 0.0], 0);
    let g = plan(&o, &bot, &[seen(4, [99.0, 1.0, 0.0], 0)], true);
    assert_eq!(g, Some(Goal::Hold(RED_HOME, DEFEND_RADIUS)));
    // Their defender, standing on its plate, gets clear the way it faces
    // out: along the line from the bomb to it.
    let o = assault(5, vec![ours], 1 << 0);
    let defender = me([98.0, -3.0, 0.0], 1);
    match plan(&o, &defender, &[], true) {
        Some(Goal::Hold(p, r)) => {
            assert_eq!(r, CLEAR_HOLD);
            assert!(dist_xy(p, BLUE_HOME) >= BLAST_CLEAR, "{p:?}");
            assert!(p[0] < BLUE_HOME[0] && p[1] < 0.0, "{p:?}");
        }
        g => panic!("{g:?}"),
    }
}

#[test]
fn an_escorting_name_covers_the_nearer_bomb_runner() {
    let bomb = flag(0, [12.0, 0.0, 0.0], None);
    let o = assault(1, vec![bomb], 0);
    let runner = seen(2, [14.0, 0.0, 0.0], 0);
    // 1 and 2 attack; 5 defends.
    let others = [runner, seen(5, [1.0, 0.0, 0.0], 0)];
    let bot = me([30.0, 0.0, 0.0], 0);
    assert_eq!(
        plan_with(&o, &bot, &others, true, &leaning(ESCORT), Temper::default()),
        Some(Goal::Escort(runner.position))
    );
    assert_eq!(
        plan_with(&o, &bot, &others, true, &leaning(RUNNER), Temper::default()),
        Some(Goal::Go(bomb.position))
    );
}

#[test]
fn a_loose_flag_lying_a_step_up_is_in_reach() {
    let o = ctf(1, vec![flag(1, [60.0, 0.0, 0.6], None)]);
    let goal = Some(Goal::Go([60.0, 0.0, 0.6]));
    assert!(o.take_at(goal, me([60.2, 0.0, 0.0], 0)));
    assert!(!o.take_at(goal, me([60.2, 0.0, -1.5], 0)));
    assert!(!o.take_at(goal, me([61.0, 0.0, 0.6], 0)));
}
