use super::super::{Game, Item, Outlook, State, plan_with};
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
fn game(game: Game, me: u32, items: Vec<Item>) -> Objective {
    Objective {
        game,
        state: Default::default(),
        me,
        items,
        spots: vec![],
        hill: None,
    }
}
fn ball(at: Vec3, carrier: Option<u32>) -> Item {
    Item {
        id: 0xe100_0200,
        team: 0,
        position: at,
        carrier,
    }
}
fn square(c: Vec3, half: f32) -> Hill {
    Hill {
        corners: vec![
            [c[0] - half, c[1] - half, c[2]],
            [c[0] + half, c[1] - half, c[2]],
            [c[0] + half, c[1] + half, c[2]],
            [c[0] - half, c[1] + half, c[2]],
        ],
        ..Default::default()
    }
}
const CALM: Temper = Temper {
    patience: 0.5,
    caution: 0.5,
};

#[test]
fn a_loose_ball_draws_the_nearest_two_and_the_rest_cover_it() {
    let at = [20.0, 0.0, 0.0];
    let o = |id| game(Game::Oddball, id, vec![ball(at, None)]);
    // Team 0: bots 1, 3, 5 at 5, 10 and 15 from the ball.
    let team = [
        seen(1, [15.0, 0.0, 0.0], 0),
        seen(3, [10.0, 0.0, 0.0], 0),
        seen(5, [5.0, 0.0, 0.0], 0),
    ];
    let goal = |bot: Seen| {
        let others: Vec<Seen> = team.iter().copied().filter(|s| s.id != bot.id).collect();
        plan_with(&o(bot.id), &me(bot.position, 0), &others, true, &Outlook::default(), CALM)
    };
    assert_eq!(goal(team[0]), Some(Goal::Go(at)));
    assert_eq!(goal(team[1]), Some(Goal::Go(at)));
    assert_eq!(goal(team[2]), Some(Goal::Hold(at, BALL_COVER)));
    // Alone (or in free-for-all) everyone runs for it.
    assert_eq!(
        plan_with(&o(5), &me(team[2].position, 0), &team[..2], false, &Outlook::default(), CALM),
        Some(Goal::Go(at))
    );
}

#[test]
fn a_hunter_ahead_of_its_team_waits_for_them_then_they_push_together() {
    let carrier = [30.0, 0.0, 0.0];
    let o = game(Game::Oddball, 1, vec![ball(carrier, Some(9))]);
    let lead = me([18.0, 0.0, 0.0], 0);
    let hunt = Some(Goal::Hunt(9, carrier));
    // A teammate 28 behind the carrier: coming, and far enough back.
    let mate = seen(2, [2.0, 0.0, 0.0], 0);
    assert_eq!(
        plan_with(&o, &lead, &[mate], true, &Outlook::default(), CALM),
        Some(Goal::Hold(lead.position, STAGE_HOLD))
    );
    // It closes up: they go together.
    let near = seen(2, [12.0, 0.0, 0.0], 0);
    assert_eq!(plan_with(&o, &lead, &[near], true, &Outlook::default(), CALM), hunt);
    // Respawning across the map is not waited for.
    let far = seen(2, [-40.0, 0.0, 0.0], 0);
    assert_eq!(plan_with(&o, &lead, &[far], true, &Outlook::default(), CALM), hunt);
    // Already on the carrier: no waiting.
    let close = me([26.0, 0.0, 0.0], 0);
    assert_eq!(
        plan_with(&o, &close, &[mate], true, &Outlook::default(), CALM),
        Some(Goal::Hunt(9, carrier))
    );
    // The impatient go on with a teammate 10 behind; the patient wait.
    let behind = seen(2, [7.0, 0.0, 0.0], 0);
    let impatient = Temper {
        patience: 0.0,
        ..CALM
    };
    let patient = Temper {
        patience: 1.0,
        ..CALM
    };
    assert_eq!(plan_with(&o, &lead, &[behind], true, &Outlook::default(), impatient), hunt);
    assert_eq!(
        plan_with(&o, &lead, &[behind], true, &Outlook::default(), patient),
        Some(Goal::Hold(lead.position, STAGE_HOLD))
    );
}

#[test]
fn escorts_screen_the_carrier_towards_the_threat_each_in_its_own_spot() {
    let carrier = [0.0, 0.0, 0.0];
    let foe = seen(7, [20.0, 0.0, 0.0], 1);
    let mates = [seen(2, carrier, 0), seen(3, [-5.0, 2.0, 0.0], 0)];
    let o = |id| game(Game::Oddball, id, vec![ball(carrier, Some(2))]);
    let spot = |id: u32, others: &[Seen]| match plan_with(
        &o(id),
        &me([-4.0, -4.0, 0.0], 0),
        others,
        true, &Outlook::default(),
        CALM,
    ) {
        Some(Goal::Escort(at)) => at,
        g => panic!("{g:?}"),
    };
    let first = spot(1, &[mates[0], mates[1], foe]);
    let second = spot(3, &[mates[0], seen(1, [-4.0, -4.0, 0.0], 0), foe]);
    // Both stand out towards the enemy, not on the carrier, and apart.
    for at in [first, second] {
        assert!(at[0] > 2.0, "towards the threat: {at:?}");
        assert!(dist(at, carrier) <= SCREEN + 1e-3);
    }
    assert!(dist(first, second) > 1.5, "{first:?} {second:?}");
    // With no threat about, by the carrier.
    assert_eq!(spot(1, &mates), carrier);
}

#[test]
fn hill_holders_keep_more_of_it_the_more_cautious_they_are() {
    let mut o = game(Game::Koth, 1, vec![]);
    o.hill = Some(square([0.0; 3], 3.0));
    let r = |caution| match plan_with(
        &o,
        &me([1.0, 0.0, 0.0], 0),
        &[],
        true,
        &Outlook::default(),
        Temper { caution, ..CALM },
    ) {
        Some(Goal::Hold(c, r)) => {
            assert_eq!(c, [0.0; 3]);
            r
        }
        g => panic!("{g:?}"),
    };
    let (bold, careful) = (r(0.0), r(1.0));
    assert!(bold < careful, "{bold} {careful}");
    // Both keep inside the hill (its corners at 4.24 from the middle).
    assert!(careful < 3.0, "{careful}");
}

#[test]
fn with_the_hill_held_the_cautious_cover_its_edge_and_a_contest_draws_everyone_in() {
    let mut o = game(Game::Koth, 1, vec![]);
    o.hill = Some(square([0.0; 3], 3.0));
    let out = me([10.0, 0.0, 0.0], 0);
    let holders = [seen(2, [1.0, 0.0, 0.0], 0), seen(3, [-1.0, 1.0, 0.0], 0)];
    let careful = Temper {
        caution: 0.8,
        ..CALM
    };
    let bold = Temper {
        caution: 0.2,
        ..CALM
    };
    match plan_with(&o, &out, &holders, true, &Outlook::default(), careful) {
        Some(Goal::Hold(c, r)) => {
            assert_eq!(c, [0.0; 3]);
            assert!(
                r > o.hill.as_ref().unwrap().radius(),
                "outside the hill: {r}"
            );
        }
        g => panic!("{g:?}"),
    }
    assert_eq!(
        plan_with(&o, &out, &holders, true, &Outlook::default(), bold),
        Some(Goal::Go([0.0; 3]))
    );
    // An enemy in it: the cautious go in to clear it too.
    let contested = [holders[0], holders[1], seen(8, [0.0, -1.0, 0.0], 1)];
    assert_eq!(
        plan_with(&o, &out, &contested, true, &Outlook::default(), careful),
        Some(Goal::Go([0.0; 3]))
    );
    // The engine's mask says who is in it.
    o.state = State::Hill { occupants: 1 << 2 };
    assert_eq!(
        plan_with(&o, &out, &holders, true, &Outlook::default(), careful),
        Some(Goal::Go([0.0; 3]))
    );
}

fn site(at: Vec3, owner_team: Option<i16>, occupants: u16) -> Territory {
    Territory {
        at,
        owner: owner_team.map(|_| 0),
        owner_team,
        occupants,
    }
}

#[test]
fn a_team_spreads_over_the_territories_instead_of_stacking_on_one() {
    let a = [10.0, 0.0, 0.0];
    let b = [14.0, 6.0, 0.0];
    let sites = vec![site(a, None, 0), site(b, None, 0)];
    let o = |id| Objective {
        state: State::Territories(sites.clone()),
        ..game(Game::Territories, id, vec![])
    };
    // Both nearer A; bot 1 the nearer, so bot 2 takes B.
    let one = seen(1, [8.0, 0.0, 0.0], 0);
    let two = seen(2, [6.0, 1.0, 0.0], 0);
    assert_eq!(
        plan_with(&o(1), &me(one.position, 0), &[two], true, &Outlook::default(), CALM),
        Some(Goal::Go(a))
    );
    assert_eq!(
        plan_with(&o(2), &me(two.position, 0), &[one], true, &Outlook::default(), CALM),
        Some(Goal::Go(b))
    );
}

#[test]
fn a_held_territory_under_pressure_draws_two_and_the_taker_holds_its_marker() {
    let ours = [0.0, 0.0, 0.0];
    let theirs = [5.0, 0.0, 0.0];
    let foe = seen(9, ours, 1);
    let sites = vec![site(ours, Some(0), 1 << 9), site(theirs, Some(1), 0)];
    let o = |id| Objective {
        state: State::Territories(sites.clone()),
        ..game(Game::Territories, id, vec![])
    };
    let team = [
        seen(1, [3.0, 0.0, 0.0], 0),
        seen(2, [4.0, 0.0, 0.0], 0),
        seen(3, [6.0, 0.0, 0.0], 0),
    ];
    let goal = |bot: Seen| {
        let mut others: Vec<Seen> = team.iter().copied().filter(|s| s.id != bot.id).collect();
        others.push(foe);
        plan_with(&o(bot.id), &me(bot.position, 0), &others, true, &Outlook::default(), CALM)
    };
    assert_eq!(goal(team[0]), Some(Goal::Go(ours)));
    assert_eq!(goal(team[1]), Some(Goal::Go(ours)));
    assert_eq!(goal(team[2]), Some(Goal::Go(theirs)));
    // Standing in a site it is taking: it holds by the marker.
    let mut taking = sites.clone();
    taking[1].occupants = 1 << 3;
    let o = Objective {
        state: State::Territories(taking),
        ..game(Game::Territories, 3, vec![])
    };
    assert_eq!(
        plan_with(&o, &me([5.5, 0.0, 0.0], 0), &[foe], true, &Outlook::default(), CALM),
        Some(Goal::Hold(theirs, TERRITORY_KEEP))
    );
}

#[test]
fn markers_alone_still_give_territories_to_take_and_hold() {
    let markers = vec![[0.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
    let o = Objective {
        state: State::Markers(markers.clone()),
        ..game(Game::Territories, 1, vec![])
    };
    let sites = sites_from_markers(
        &markers,
        1,
        [1.0, 1.0, 0.0],
        &[seen(4, [19.0, 0.0, 3.0], 1)],
    );
    assert_eq!(sites[0].occupants, 1 << 1);
    assert_eq!(sites[1].occupants, 0, "a floor above is not in it");
    assert_eq!(
        plan_with(&o, &me([1.0, 1.0, 0.0], 0), &[], true, &Outlook::default(), CALM),
        Some(Goal::Hold([0.0; 3], TERRITORY_KEEP))
    );
    assert_eq!(
        plan_with(&o, &me([15.0, 0.0, 0.0], 0), &[], true, &Outlook::default(), CALM),
        Some(Goal::Go([20.0, 0.0, 0.0]))
    );
}

#[test]
fn footwork_in_a_zone_never_steps_out_of_it() {
    let anchor = [0.0; 3];
    // In the middle: as it was.
    assert_eq!(within([1.0, 0.0], [0.5, 0.0, 0.0], anchor, 2.0), [1.0, 0.0]);
    // Near the edge: the outward part goes, the sideways part stays.
    let w = within([0.6, 0.8], [1.6, 0.0, 0.0], anchor, 2.0);
    assert!(w[0].abs() < 1e-6 && (w[1] - 0.8).abs() < 1e-6, "{w:?}");
    // Stepping back in is left alone.
    assert_eq!(
        within([-1.0, 0.0], [1.6, 0.0, 0.0], anchor, 2.0),
        [-1.0, 0.0]
    );
}

#[test]
fn without_engine_globals_the_flags_give_the_first_hill_and_the_ball_spawns() {
    let flags = [
        ([0.0; 3], kind::BALL_SPAWN, 8),
        ([1.0; 3], kind::BALL_SPAWN, 0),
        ([9.0, 0.0, 0.0], kind::HILL + 1, 8),
        ([9.0, 4.0, 0.0], kind::HILL + 1, 8),
        ([13.0, 4.0, 0.0], kind::HILL + 1, 8),
        ([30.0, 0.0, 0.0], kind::HILL + 2, 8),
    ];
    assert_eq!(ball_spawns(&flags), vec![[0.0; 3]]);
    let h = first_hill(&flags, &[]).expect("hill 1");
    assert_eq!(h.corners.len(), 3);
    assert!(first_hill(&flags[..2], &[]).is_none());
}

/// A flag with an area is the hill, and the area decides who is in it, a
/// notch of a concave path included (docs/game/gametype-markers.md, "In a
/// script").
#[test]
fn a_flag_with_an_area_is_the_first_hill_and_the_area_is_its_zone() {
    use game_markers::Area;
    let flags = [
        ([0.0; 3], kind::BALL_SPAWN, 8),
        ([20.0, 0.0, 0.0], kind::HILL + 1, 8),
        ([30.0, 0.0, 0.0], kind::HILL + 2, 8),
    ];
    // An L: the notch is the corner over x > 0, y > 0 (offsets).
    let l = Area::path(vec![[-4.0, -4.0], [4.0, -4.0], [4.0, 0.0], [0.0, 0.0], [0.0, 4.0], [-4.0, 4.0]]);
    assert_eq!(l.validate(), Ok(()));
    let areas = [(1, l.clone())];
    let h = first_hill(&flags, &areas).expect("hill 1");
    assert_eq!(h.corners, vec![[20.0, 0.0, 0.0]]);
    assert!(h.contains([18.0, -2.0, 0.0]));
    assert!(h.contains([18.0, 2.0, 0.0]));
    assert!(!h.contains([22.0, 2.0, 0.0]), "the notch");
    assert!(!h.contains([18.0, -2.0, 3.0]), "over its reach");
    assert_eq!(h.center(), l.middle([20.0, 0.0, 0.0]));
    assert!(h.radius() > 0.0);
    // An area on a later hill's flag makes that the first hill; with no
    // area a lone flag is no hill.
    assert!(first_hill(&flags, &[(2, l)]).is_some_and(|h| h.area.is_some()));
    assert!(first_hill(&flags, &[]).is_none());
}

#[test]
fn a_names_temper_is_its_own_every_match() {
    use crate::mind::Skill;
    use crate::name_profile::NameProfile;
    let of = |name| Temper::from(NameProfile::for_name(name).personality(Skill::Legendary));
    assert_eq!(of("Kestrel"), of("kestrel"));
    let t = of("Moth");
    assert!((0.0..=1.0).contains(&t.patience) && (0.0..=1.0).contains(&t.caution));
    assert_ne!(of("Kestrel"), of("Moth"));
}
