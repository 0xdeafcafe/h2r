use super::*;

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

/// Red (team 0) home at X 0, blue (team 1) home at X 100; both flags home.
const RED_HOME: Vec3 = [0.0, 0.0, 0.0];
const BLUE_HOME: Vec3 = [100.0, 0.0, 0.0];
fn ctf(me_id: u32, items: Vec<Item>) -> Objective {
    Objective {
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

#[test]
fn roles_split_a_team_the_same_way_in_every_bot() {
    // One alone attacks; two split; three: one defends; six: two.
    assert_eq!(role(0, 1), Role::Attack);
    assert_eq!((role(0, 2), role(1, 2)), (Role::Attack, Role::Defend));
    let three: Vec<Role> = (0..3).map(|r| role(r, 3)).collect();
    assert_eq!(three, [Role::Attack, Role::Attack, Role::Defend]);
    let six = (0..6).filter(|&r| role(r, 6) == Role::Defend).count();
    assert_eq!(six, 2);
    // By index, whoever asks: the bots 2, 5 and 9 agree.
    let team = [2, 5, 9];
    let roles: Vec<Role> = team
        .iter()
        .map(|&id| {
            let mates: Vec<u32> = team.iter().copied().filter(|&m| m != id).collect();
            role_of(id, &mates)
        })
        .collect();
    assert_eq!(roles, [Role::Attack, Role::Attack, Role::Defend]);
}

#[test]
fn with_both_flags_home_attackers_go_for_theirs_and_defenders_guard_ours() {
    let o = ctf(1, home_flags());
    let mate = seen(4, [5.0, 0.0, 0.0], 0);
    // Bot 1 with mate 4: rank 0 of 2, attacks.
    let g = plan(&o, &me([10.0, 0.0, 0.0], 0), &[mate], true);
    assert_eq!(g, Some(Goal::Go(BLUE_HOME)));
    // Bot 4 with mate 1: rank 1 of 2, defends.
    let o = ctf(4, home_flags());
    let g = plan(
        &o,
        &me([5.0, 0.0, 0.0], 0),
        &[seen(1, [10.0, 0.0, 0.0], 0)],
        true,
    );
    assert_eq!(g, Some(Goal::Hold(RED_HOME, DEFEND_RADIUS)));
}

#[test]
fn the_carrier_runs_home_to_score() {
    let mut items = home_flags();
    items[1] = flag(1, [60.0, 0.0, 0.0], Some(1));
    let o = ctf(1, items.clone());
    let g = plan(&o, &me([60.0, 0.0, 0.0], 0), &[], true);
    assert_eq!(g, Some(Goal::Carry(RED_HOME)));
    // A return point, when the map has one, is where it scores.
    let mut o = ctf(1, items);
    o.spots.push(spot(kind::CTF_RETURN, 0, [2.0, 3.0, 0.0]));
    o.spots.push(spot(kind::CTF_RETURN, 1, [98.0, 3.0, 0.0]));
    let g = plan(&o, &me([60.0, 0.0, 0.0], 0), &[], true);
    assert_eq!(g, Some(Goal::Carry([2.0, 3.0, 0.0])));
}

#[test]
fn a_teammate_with_their_flag_is_escorted_by_attackers() {
    let mut items = home_flags();
    items[1] = flag(1, [70.0, 0.0, 0.0], Some(4));
    let o = ctf(1, items.clone());
    let mate = seen(4, [70.0, 0.0, 0.0], 0);
    let g = plan(&o, &me([50.0, 0.0, 0.0], 0), &[mate], true);
    assert_eq!(g, Some(Goal::Escort([70.0, 0.0, 0.0])));
    // A defender (rank 2 of 3) stays home.
    let o = ctf(9, items);
    let mates = [mate, seen(1, [50.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([3.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Hold(RED_HOME, DEFEND_RADIUS)));
}

#[test]
fn an_enemy_with_our_flag_is_hunted() {
    let mut items = home_flags();
    items[0] = flag(0, [30.0, 0.0, 0.0], Some(7));
    // The defender hunts wherever it is.
    let o = ctf(4, items.clone());
    let mates = [seen(1, [90.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([2.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Hunt(7, [30.0, 0.0, 0.0])));
    // An attacker nearer the carrier than their flag hunts too...
    let o = ctf(1, items.clone());
    let mates = [seen(4, [2.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([40.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Hunt(7, [30.0, 0.0, 0.0])));
    // ...and one nearer their flag goes on for it.
    let g = plan(&o, &me([90.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Go(BLUE_HOME)));
}

#[test]
fn our_dropped_flag_is_returned() {
    let mut items = home_flags();
    items[0] = flag(0, [20.0, 5.0, 0.0], None);
    // The defender returns it from anywhere.
    let o = ctf(4, items.clone());
    let mates = [seen(1, [90.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([0.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Go([20.0, 5.0, 0.0])));
    // An attacker only when near it.
    let o = ctf(1, items.clone());
    let mates = [seen(4, [0.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([30.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Go([20.0, 5.0, 0.0])));
    let g = plan(&o, &me([80.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Go(BLUE_HOME)));
    // At home (within HOME of its spawn) it is not "dropped".
    let o = ctf(
        4,
        vec![flag(0, [1.0, 0.0, 0.0], None), flag(1, BLUE_HOME, None)],
    );
    let mates = [seen(1, [90.0, 0.0, 0.0], 0)];
    let g = plan(&o, &me([0.0, 0.0, 0.0], 0), &mates, true);
    assert_eq!(g, Some(Goal::Hold(RED_HOME, DEFEND_RADIUS)));
}

#[test]
fn their_dropped_flag_is_taken_where_it_lies() {
    let mut items = home_flags();
    items[1] = flag(1, [55.0, -4.0, 0.0], None);
    let o = ctf(1, items);
    let g = plan(&o, &me([10.0, 0.0, 0.0], 0), &[], true);
    assert_eq!(g, Some(Goal::Go([55.0, -4.0, 0.0])));
}

fn oddball(me_id: u32, items: Vec<Item>) -> Objective {
    Objective {
        game: Game::Oddball,
        me: me_id,
        items,
        spots: vec![],
        hill: None,
    }
}

#[test]
fn oddball_free_ball_is_fetched_and_a_held_one_hunted_or_kept() {
    let ball = |carrier| Item {
        id: 0xe100_0200,
        team: 0,
        position: [12.0, 3.0, 0.0],
        carrier,
    };
    let at = me([0.0; 3], 0);
    let others = [seen(2, [12.0, 3.0, 0.0], 1), seen(3, [1.0, 0.0, 0.0], 0)];
    // Free: everyone goes for it.
    let g = plan(&oddball(1, vec![ball(None)]), &at, &others, true);
    assert_eq!(g, Some(Goal::Go([12.0, 3.0, 0.0])));
    // Mine: keep it.
    let g = plan(&oddball(1, vec![ball(Some(1))]), &at, &others, true);
    assert_eq!(g, Some(Goal::Keep));
    // An enemy's: hunt them.
    let g = plan(&oddball(1, vec![ball(Some(2))]), &at, &others, true);
    assert_eq!(g, Some(Goal::Hunt(2, [12.0, 3.0, 0.0])));
    // A teammate's: escort.
    let g = plan(&oddball(1, vec![ball(Some(3))]), &at, &others, true);
    assert_eq!(g, Some(Goal::Escort([12.0, 3.0, 0.0])));
    // Free-for-all: everyone else's is an enemy's.
    let g = plan(&oddball(1, vec![ball(Some(3))]), &at, &others, false);
    assert_eq!(g, Some(Goal::Hunt(3, [12.0, 3.0, 0.0])));
}

fn square(c: Vec3, half: f32) -> Hill {
    Hill {
        // Not in ring order: the scenario's order is whatever it is.
        corners: vec![
            [c[0] - half, c[1] - half, c[2]],
            [c[0] + half, c[1] + half, c[2]],
            [c[0] + half, c[1] - half, c[2]],
            [c[0] - half, c[1] + half, c[2]],
        ],
    }
}

#[test]
fn the_hill_is_inside_its_corners_and_near_its_height() {
    let h = square([10.0, 10.0, 2.0], 3.0);
    assert_eq!(h.center(), [10.0, 10.0, 2.0]);
    assert!(h.contains([10.0, 10.0, 2.5]));
    assert!(h.contains([12.5, 7.5, 2.0]));
    assert!(!h.contains([13.5, 10.0, 2.0]), "outside a side");
    assert!(!h.contains([10.0, 10.0, 6.0]), "a floor above");
    assert!(!Hill::default().contains([0.0; 3]));
}

#[test]
fn koth_goes_to_the_hill_stays_in_it_and_follows_it_when_it_moves() {
    let mut o = Objective {
        game: Game::Koth,
        me: 1,
        items: vec![],
        spots: vec![],
        hill: Some(square([10.0, 10.0, 0.0], 3.0)),
    };
    let away = me([40.0, 0.0, 0.0], 0);
    assert_eq!(
        plan(&o, &away, &[], false),
        Some(Goal::Go([10.0, 10.0, 0.0]))
    );
    let inside = me([11.0, 9.0, 0.0], 0);
    match plan(&o, &inside, &[], false) {
        Some(Goal::Hold(c, r)) => {
            assert_eq!(c, [10.0, 10.0, 0.0]);
            assert!(r > 0.5 && r < 3.0, "{r}");
        }
        g => panic!("{g:?}"),
    }
    // The hill moves: off to the new one.
    o.hill = Some(square([-20.0, 5.0, 0.0], 2.0));
    assert_eq!(
        plan(&o, &inside, &[], false),
        Some(Goal::Go([-20.0, 5.0, 0.0]))
    );
    // No hill read: nothing asked.
    o.hill = None;
    assert_eq!(plan(&o, &inside, &[], false), None);
}

#[test]
fn territories_ask_nothing_yet() {
    let o = Objective {
        game: Game::Territories,
        me: 1,
        items: vec![],
        spots: vec![],
        hill: None,
    };
    assert_eq!(plan(&o, &me([0.0; 3], 0), &[], true), None);
}

#[test]
fn engine_indices_name_the_objective_games_only() {
    assert_eq!(Game::from_engine(1), Some(Game::Ctf));
    assert_eq!(Game::from_engine(2), None, "Slayer");
    assert_eq!(Game::from_engine(3), Some(Game::Oddball));
    assert_eq!(Game::from_engine(4), Some(Game::Koth));
    assert_eq!(Game::from_engine(7), None, "Juggernaut");
    assert_eq!(Game::from_engine(8), Some(Game::Territories));
    assert_eq!(Game::from_engine(9), Some(Game::Assault));
}

#[test]
fn the_last_teammate_escorts_a_flag_once_our_drop_is_secure() {
    let mate = seen(1, [65.0, 0.0, 0.0], 0);
    let escort = flag(1, mate.position, Some(mate.id));
    let bot = me([45.0, 0.0, 0.0], 0);
    let dropped = flag(0, [10.0, 0.0, 0.0], None);
    assert_eq!(
        plan(&ctf(4, vec![dropped, escort]), &bot, &[mate], true),
        Some(Goal::Go(dropped.position))
    );
    assert_eq!(
        plan(
            &ctf(4, vec![flag(0, RED_HOME, None), escort]),
            &bot,
            &[mate],
            true
        ),
        Some(Goal::Escort(mate.position))
    );
}

fn assault(id: u32, items: Vec<Item>) -> Objective {
    Objective {
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
fn assault_attackers_take_the_friendly_or_neutral_bomb_to_the_enemy_plate() {
    let bot = me([22.0, 0.0, 0.0], 0);
    let enemy_drop = flag(1, [23.0, 0.0, 0.0], None);
    for team in [0, 8] {
        let bomb = flag(team, [12.0, 0.0, 0.0], None);
        assert_eq!(
            plan(&assault(1, vec![enemy_drop, bomb]), &bot, &[], true),
            Some(Goal::Go(bomb.position))
        );
        let carrying = Item {
            carrier: Some(1),
            ..bomb
        };
        assert_eq!(
            plan(&assault(1, vec![carrying]), &bot, &[], true),
            Some(Goal::Carry(BLUE_HOME))
        );
    }
}

#[test]
fn assault_escorts_protect_the_plant_and_defenders_intercept_the_incoming_carrier() {
    let mate = seen(1, [62.0, 0.0, 0.0], 0);
    let theirs = seen(8, [10.0, 0.0, 0.0], 1);
    let bot = me([10.0, 0.0, 0.0], 0);
    let ally_bomb = flag(0, mate.position, Some(mate.id));
    let hostile_bomb = flag(1, theirs.position, Some(theirs.id));
    let goal = |items| plan(&assault(4, items), &bot, &[mate, theirs], true);
    assert_eq!(goal(vec![ally_bomb]), Some(Goal::Escort(mate.position)));
    assert_eq!(
        goal(vec![Item {
            position: [98.0, 1.0, 0.0],
            ..ally_bomb
        }]),
        Some(Goal::Hold(BLUE_HOME, DEFEND_RADIUS))
    );
    assert_eq!(
        goal(vec![ally_bomb, hostile_bomb]),
        Some(Goal::Hunt(theirs.id, theirs.position))
    );
    assert_eq!(
        goal(vec![flag(1, [70.0, 0.0, 0.0], None)]),
        Some(Goal::Hold(RED_HOME, DEFEND_RADIUS))
    );
}

#[test]
fn when_the_home_defender_takes_the_flag_another_teammate_guards() {
    let carrier = seen(9, [72.0, 0.0, 0.0], 0);
    let o = ctf(
        4,
        vec![flag(0, RED_HOME, None), flag(1, carrier.position, Some(9))],
    );
    let mates = [carrier, seen(1, [40.0, 0.0, 0.0], 0)];
    assert_eq!(
        plan(&o, &me([35.0, 0.0, 0.0], 0), &mates, true),
        Some(Goal::Hold(RED_HOME, DEFEND_RADIUS))
    );
}

#[test]
fn one_attacker_recovers_a_bomb_dropped_at_the_plant_while_the_rest_cover() {
    let b = flag(0, [99.0, 0.0, 0.0], None);
    let mate = seen(4, [97.0, 1.0, 0.0], 0);
    let p = me([80.0, 0.0, 0.0], 0);
    assert_eq!(
        plan(&assault(1, vec![b]), &p, &[mate], true),
        Some(Goal::Hold(BLUE_HOME, DEFEND_RADIUS))
    );
    assert_eq!(
        plan(
            &assault(4, vec![b]),
            &me(mate.position, 0),
            &[seen(1, p.position, 0)],
            true
        ),
        Some(Goal::Go(b.position))
    );
}

#[test]
fn an_unreadable_scoring_plate_keeps_the_carrier_in_possession() {
    let p = me([30.0, 4.0, 2.0], 0);
    for game in [Game::Ctf, Game::Assault] {
        let o = Objective {
            game,
            me: 1,
            items: vec![flag(1, p.position, Some(1))],
            spots: vec![],
            hill: None,
        };
        assert_eq!(plan(&o, &p, &[], true), Some(Goal::Carry(p.position)));
    }
}
