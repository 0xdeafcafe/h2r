//! A script's goals as Siege sets them: the attackers take the site, the
//! defenders hold it and hunt who comes in.

use super::*;

const SITE: Vec3 = [50.0, 0.0, 0.0];
const STAND: Vec3 = [90.0, 0.0, 0.0];
const MIDDLE: Vec3 = [50.0, 0.0, 0.5];

fn goal(team: i16, aim: Aim, zone: u32, at: Vec3) -> ScriptGoal {
    ScriptGoal {
        team,
        aim,
        zone,
        at,
        yaw: 0.0,
        area: game_markers::Area::radius(4.0),
    }
}

/// Siege's first phase: team 0 attacks the site, team 1 defends it.
fn siege(me: u32) -> ScriptGoals {
    ScriptGoals {
        me,
        list: vec![goal(0, Aim::Attack, 10, SITE), goal(1, Aim::Defend, 10, SITE)],
    }
}

fn me(at: Vec3, team: i16) -> Me {
    Me {
        position: at,
        team,
        ..Me::default()
    }
}

fn seen(id: u32, at: Vec3, team: i16) -> Seen {
    Seen { id, position: at, team }
}

fn plan_for(goals: &ScriptGoals, at: Vec3, team: i16, others: &[Seen]) -> Option<Goal> {
    plan(goals, &me(at, team), others, true, Temper::default())
}

#[test]
fn attackers_head_to_the_site_and_keep_in_it() {
    let mates = [seen(1, [0.0, 2.0, 0.0], 0), seen(2, [0.0, 4.0, 0.0], 0)];
    // Every attacker goes, the highest rank too: the team has no place to
    // defend, so nobody stays back.
    for id in [0, 2] {
        assert_eq!(plan_for(&siege(id), [0.0; 3], 0, &mates), Some(Goal::Go(MIDDLE)));
    }
    match plan_for(&siege(0), [51.0, 1.0, 0.0], 0, &mates) {
        Some(Goal::Hold(at, r)) => {
            assert_eq!(at, MIDDLE);
            assert!(r > 0.5 && r < 4.0, "keeps about the middle, inside: {r}");
        }
        other => panic!("in the site it holds: {other:?}"),
    }
}

#[test]
fn defenders_hold_the_site_and_hunt_who_comes_in() {
    let mates = [seen(4, [95.0, 0.0, 0.0], 1)];
    assert_eq!(plan_for(&siege(3), STAND, 1, &mates), Some(Goal::Go(MIDDLE)));
    assert!(matches!(
        plan_for(&siege(3), [50.0, 1.0, 0.0], 1, &mates),
        Some(Goal::Hold(MIDDLE, _))
    ));
    // An attacker near the site is hunted; one far off is not.
    let near = [mates[0], seen(0, [56.0, 0.0, 0.0], 0)];
    assert_eq!(
        plan_for(&siege(3), [50.0, 1.0, 0.0], 1, &near),
        Some(Goal::Hunt(0, [56.0, 0.0, 0.0]))
    );
    let far = [mates[0], seen(0, [0.0, 0.0, 0.0], 0)];
    assert!(matches!(
        plan_for(&siege(3), [50.0, 1.0, 0.0], 1, &far),
        Some(Goal::Hold(MIDDLE, _))
    ));
}

#[test]
fn a_team_with_no_goal_plays_the_retail_objective() {
    let goals = ScriptGoals {
        me: 0,
        list: vec![goal(1, Aim::Defend, 10, SITE)],
    };
    assert_eq!(plan_for(&goals, [0.0; 3], 0, &[]), None);
    assert_eq!(plan_for(&ScriptGoals::default(), [0.0; 3], 0, &[]), None);
}

#[test]
fn attack_and_defend_split_the_team_by_role() {
    // Team 0 attacks the stand and defends its own site: of three, the
    // highest rank defends (`role`).
    let list = vec![goal(0, Aim::Attack, 22, STAND), goal(0, Aim::Defend, 10, SITE)];
    let mates = [seen(1, [0.0; 3], 0), seen(2, [0.0; 3], 0)];
    let at = |id| plan_for(&ScriptGoals { me: id, list: list.clone() }, [0.0; 3], 0, &mates);
    assert_eq!(at(0), Some(Goal::Go([90.0, 0.0, 0.5])));
    assert_eq!(at(1), Some(Goal::Go([90.0, 0.0, 0.5])));
    assert_eq!(at(2), Some(Goal::Go(MIDDLE)));
}

#[test]
fn defenders_spread_over_the_places_and_hold_takes_everyone() {
    let list = vec![goal(1, Aim::Defend, 10, SITE), goal(1, Aim::Defend, 11, STAND)];
    // Two defenders, 4 and 5: each sees the other.
    let mates = [seen(5, [0.0; 3], 1)];
    let at = |id: u32| {
        let other = [seen(9 - id, [0.0; 3], 1)];
        plan_for(&ScriptGoals { me: id, list: list.clone() }, [0.0; 3], 1, &other)
    };
    assert_eq!(at(4), Some(Goal::Go(MIDDLE)));
    assert_eq!(at(5), Some(Goal::Go([90.0, 0.0, 0.5])));
    let mut held = list.clone();
    held.push(goal(1, Aim::Hold, 12, [0.0, 50.0, 0.0]));
    let goals = ScriptGoals { me: 5, list: held };
    assert_eq!(plan_for(&goals, [0.0; 3], 1, &mates), Some(Goal::Go([0.0, 50.0, 0.5])));
}

#[test]
fn the_table_replaces_a_zone_and_refuses_by_name() {
    let mut t = Vec::new();
    put(&mut t, goal(0, Aim::Attack, 10, SITE)).unwrap();
    put(&mut t, goal(0, Aim::Defend, 10, SITE)).unwrap();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].aim, Aim::Defend);
    for z in 11..(10 + TEAM_GOALS as u32) {
        put(&mut t, goal(0, Aim::Attack, z, SITE)).unwrap();
    }
    let full = put(&mut t, goal(0, Aim::Attack, 99, SITE)).unwrap_err();
    assert_eq!(full, Refused::Full { team: 0 });
    assert!(full.to_string().starts_with("bot goal refused: team 0 has 8 goals"));
    // Another team has room of its own.
    put(&mut t, goal(1, Aim::Defend, 99, SITE)).unwrap();
    assert_eq!(put(&mut t, goal(8, Aim::Hold, 1, SITE)), Err(Refused::NoTeam { team: 8 }));
}
