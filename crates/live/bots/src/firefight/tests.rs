//! Firefight's bots on invented matches: seats, enemies and the home base.

use super::*;
use crate::brain::Observations;
use crate::cache::MapData;
use crate::los::Walls;
use crate::mind::{Doing, Mind, Skill};
use crate::vehicle::Sight;

const HOME: Vec3 = [0.0, 0.0, 0.0];
const AWAY: Vec3 = [40.0, 0.0, 0.0];
/// An actor: [`ACTOR_ID_BASE`] plus datum index 3.
const ACTOR: u32 = ACTOR_ID_BASE + 3;

fn seen(id: u32, team: i16, position: Vec3) -> Seen {
    Seen { id, team, position }
}

fn me_at(position: Vec3, team: i16) -> Me {
    Me {
        position,
        team,
        ..Me::default()
    }
}

/// One player team on engine team 0, the AI on team 3.
fn one_team() -> Context {
    Context {
        player_teams: vec![0],
        ai_team: 3,
        bases: vec![HOME],
        lives: vec![Some(20)],
        phase: Phase::Fighting,
        drop_zones: vec![[14.0, 0.0, 0.0], [-30.0, 0.0, 0.0]],
    }
}

/// Two player teams on engine teams 0 and 1, the AI on team 2.
fn two_teams() -> Context {
    Context {
        player_teams: vec![0, 1],
        ai_team: 2,
        bases: vec![HOME, AWAY],
        lives: vec![Some(20), Some(20)],
        ..one_team()
    }
}

fn perception(me: Me, others: Vec<Seen>, teams: bool) -> Perception {
    Perception {
        me: Some(me),
        others,
        teams,
        observations: Observations {
            player: Some((0, me.team)),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A wall across +X at x = 10: in front of it is in view, behind it not.
fn walled() -> MapData {
    MapData {
        walls: Walls::new(vec![[[10., -20., -5.], [10., 20., -5.], [10., 0., 20.]]]),
        ..Default::default()
    }
}

/// The ids the bot would pick a target from.
fn targets(p: &Perception) -> Vec<u32> {
    let mut ids: Vec<u32> = Mind::visible(p, None).iter().map(|o| o.id).collect();
    ids.sort_unstable();
    ids
}

fn weights(aggression: f32, retreat: f32) -> Weights {
    let mut w = [0.5; 12];
    w[3] = aggression;
    w[11] = retreat;
    Weights::from(w)
}

#[test]
fn firefight_seats_are_player_teams_only() {
    // One player team: always team 0, whoever holds what.
    assert_eq!(seat_team([], 1), 0);
    assert_eq!(seat_team([0, 0, 0, 1, 2, 7], 1), 0);
    // Two: the one with fewer players, 0 on a tie, never a third team
    // however empty it is.
    assert_eq!(seat_team([], 2), 0);
    assert_eq!(seat_team([0], 2), 1);
    assert_eq!(seat_team([0, 1], 2), 0);
    assert_eq!(seat_team([1, 1, 0], 2), 0);
    for held in [vec![0, 1, 2, 2], vec![0, 0, 1, 1, 3], vec![5, 6, 7]] {
        assert!(seat_team(held.clone(), 2) < 2, "{held:?}");
    }
    // A bad count is clamped to the player teams Firefight has.
    assert!(seat_team([0, 1], 3) < 2);
    assert_eq!(seat_team([0], 0), 0);
    // Filling a lobby one Clanker at a time keeps the teams even.
    let mut held = vec![0u8];
    for _ in 0..5 {
        let t = seat_team(held.clone(), 2);
        held.push(t);
    }
    let reds = held.iter().filter(|&&t| t == 0).count();
    assert_eq!((reds, held.len() - reds), (3, 3));
}

#[test]
fn actors_are_seen_as_targets() {
    let ctx = one_team();
    let mut p = perception(
        me_at(HOME, 0),
        vec![seen(ACTOR, 3, [5.0, 0.0, 0.0]), seen(ACTOR + 1, 3, [15.0, 0.0, 0.0])],
        true,
    );
    normalise(&mut p, &ctx);
    // Before sight: both actors are enemies.
    assert_eq!(targets(&p), vec![ACTOR, ACTOR + 1]);
    // The sight filter keeps the one in front of the wall, as it would a
    // player.
    let map = walled();
    let mask = crate::perceive::observe_for_brain(
        &mut p,
        Skill::Legendary,
        &mut Sight::default(),
        &mut [],
        Some(&map),
        &mut Default::default(),
    );
    assert_eq!(mask, 0, "actors have no player bit");
    assert_eq!(p.others, vec![seen(ACTOR, 3, [5.0, 0.0, 0.0])]);
    // The stage-1 target and the mind's fight are the actor.
    assert_eq!(crate::brain::target(&p), Some(0));
    let mut mind = Mind::new(Skill::Legendary, 7);
    mind.set_firefight(Some(Arc::new(ctx)));
    mind.step_observed(&p, None, &[], &[], Some(mask));
    assert_eq!(mind.doing, Doing::Fight(ACTOR));
}

#[test]
fn teammates_are_never_targeted() {
    // One player team: a person on another team byte (a free-for-all
    // variant) is still a teammate; the bot's own byte is the AI's.
    let ctx = one_team();
    let mut p = perception(
        me_at(HOME, 3),
        vec![
            seen(1, 5, [3.0, 0.0, 0.0]),
            seen(2, 0, [4.0, 1.0, 0.0]),
            seen(ACTOR, 3, [6.0, 0.0, 0.0]),
        ],
        false,
    );
    normalise(&mut p, &ctx);
    assert!(p.teams);
    assert_eq!(p.me.unwrap().team, 0);
    assert_eq!(targets(&p), vec![ACTOR]);
    // Actors never share the bots' team, even with the AI on a player
    // team (a misassignment): they are enemies all the same.
    let bad = Context {
        ai_team: 0,
        ..one_team()
    };
    let mut p = perception(
        me_at(HOME, 0),
        vec![seen(1, 0, [3.0, 0.0, 0.0]), seen(ACTOR, 0, [6.0, 0.0, 0.0])],
        true,
    );
    normalise(&mut p, &bad);
    assert_eq!(p.others[1].team, ACTOR_TEAM);
    assert_eq!(targets(&p), vec![ACTOR]);
    // Teammates are kept out of sight too (their HUD markers), and are
    // never fought.
    let map = walled();
    let mut p = perception(
        me_at(HOME, 0),
        vec![seen(1, 0, [30.0, 0.0, 0.0]), seen(ACTOR, 3, [5.0, 0.0, 0.0])],
        true,
    );
    normalise(&mut p, &ctx);
    let mask = crate::perceive::observe_for_brain(
        &mut p,
        Skill::Legendary,
        &mut Sight::default(),
        &mut [],
        Some(&map),
        &mut Default::default(),
    );
    assert_eq!(p.others.len(), 2);
    let mut mind = Mind::new(Skill::Legendary, 3);
    mind.set_firefight(Some(Arc::new(ctx)));
    for tick in 0..30 {
        p.tick = tick;
        mind.step_observed(&p, None, &[], &[], Some(mask));
        assert_ne!(mind.doing, Doing::Fight(1));
    }
}

#[test]
fn two_team_bots_target_the_other_team_and_actors() {
    let ctx = two_teams();
    let others = vec![
        seen(1, 0, [3.0, 0.0, 0.0]),
        seen(2, 1, [4.0, 2.0, 0.0]),
        seen(ACTOR, 2, [6.0, 0.0, 0.0]),
    ];
    let mut red = perception(me_at(HOME, 0), others.clone(), true);
    normalise(&mut red, &ctx);
    assert_eq!(targets(&red), vec![2, ACTOR]);
    // Blue's bot: red's players and the actors.
    let mut blue = perception(me_at(HOME, 1), others, true);
    normalise(&mut blue, &ctx);
    assert_eq!(targets(&blue), vec![1, ACTOR]);
    // Two player teams keep their own team bytes.
    assert_eq!(blue.others[0].team, 0);
    assert_eq!(blue.others[2].team, 2);
    // Each team holds its own base.
    let mut state = State::default();
    state.set(Some(Arc::new(ctx)));
    let blue_me = me_at([10.0, 0.0, 0.0], 1);
    let p = perception(blue_me, vec![], true);
    assert!(matches!(state.plan(&p, &blue_me, None), Some(Goal::Hold(at, _)) if at == AWAY));
}

#[test]
fn bots_hold_near_their_home_base() {
    let mut state = State::default();
    state.set(Some(Arc::new(one_team())));
    let hold = Traits::of(None).hold;
    // In a wave: hold home, near or far.
    for at in [[1.0, 0.0, 0.0], [30.0, 0.0, 0.0]] {
        let me = me_at(at, 0);
        let p = perception(me, vec![], true);
        assert_eq!(state.plan(&p, &me, None), Some(Goal::Hold(HOME, hold)));
    }
    // Hurt: fall back, closer in.
    let hurt = Me {
        shield: 0.0,
        health: 0.3,
        ..me_at([5.0, 0.0, 0.0], 0)
    };
    let p = perception(hurt, vec![], true);
    assert_eq!(state.plan(&p, &hurt, None), Some(Goal::Hold(HOME, FALLBACK_RADIUS)));
    // Between waves: free near home (to rearm), held when out past the
    // errand radius, and not free again until back inside the hold.
    let mut state = State::default();
    state.set(Some(Arc::new(Context {
        phase: Phase::Between,
        drop_zones: vec![],
        ..one_team()
    })));
    let near = me_at([hold * 1.5, 0.0, 0.0], 0);
    let p = perception(near, vec![], true);
    assert_eq!(state.plan(&p, &near, None), None);
    let far = me_at([hold * ERRAND + 1.0, 0.0, 0.0], 0);
    let p = perception(far, vec![], true);
    assert_eq!(state.plan(&p, &far, None), Some(Goal::Hold(HOME, hold)));
    let p = perception(near, vec![], true);
    assert_eq!(state.plan(&p, &near, None), Some(Goal::Hold(HOME, hold)), "still returning");
    let home = me_at([hold * 0.5, 0.0, 0.0], 0);
    let p = perception(home, vec![], true);
    assert_eq!(state.plan(&p, &home, None), None);
    // Weapons past the errand radius are left where they lie.
    assert!(state.blocks([hold * ERRAND + 1.0, 0.0, 0.0], &p, &home, None));
    assert!(!state.blocks([hold, 0.0, 0.0], &p, &home, None));
    // The name sets the radius: a pushy name holds wider than a careful one.
    assert!(Traits::of(Some(weights(0.9, 0.5))).hold > Traits::of(Some(weights(0.1, 0.5))).hold);
    // ...and a careful one falls back sooner.
    assert!(Traits::of(Some(weights(0.5, 0.9))).hurt > Traits::of(Some(weights(0.5, 0.1))).hurt);
    // Outside Firefight, or with no base for its team: nothing asked.
    let me = me_at(AWAY, 0);
    let p = perception(me, vec![], true);
    assert_eq!(State::default().plan(&p, &me, None), None);
    let mut no_base = State::default();
    no_base.set(Some(Arc::new(Context {
        bases: vec![],
        ..one_team()
    })));
    assert_eq!(no_base.plan(&p, &me, None), None);
    // A bot on the AI's team (never seated so) asks nothing of Firefight.
    let mut state = State::default();
    state.set(Some(Arc::new(two_teams())));
    let stray = me_at(AWAY, 2);
    assert_eq!(state.plan(&perception(stray, vec![], true), &stray, None), None);
}

#[test]
fn bots_regroup_when_the_pool_runs_low() {
    let ctx = |lives| Context {
        lives: vec![lives],
        phase: Phase::Between,
        ..one_team()
    };
    let me = me_at([3.0, 0.0, 0.0], 0);
    let mate = seen(1, 0, [20.0, 0.0, 0.0]);
    let p = perception(me, vec![mate, seen(ACTOR, 3, [9.0, 0.0, 0.0])], true);
    let mut state = State::default();
    // Plenty of lives: free between waves.
    state.set(Some(Arc::new(ctx(Some(10)))));
    assert_eq!(state.plan(&p, &me, None), None);
    // Two living players and two lives left: regroup on home, even
    // between waves. Actors are not players.
    state.set(Some(Arc::new(ctx(Some(2)))));
    assert_eq!(state.plan(&p, &me, None), Some(Goal::Hold(HOME, REGROUP_RADIUS)));
    state.set(Some(Arc::new(ctx(Some(0)))));
    assert_eq!(state.plan(&p, &me, None), Some(Goal::Hold(HOME, REGROUP_RADIUS)));
    // Unlimited lives never regroup.
    state.set(Some(Arc::new(ctx(None))));
    assert_eq!(state.plan(&p, &me, None), None);
    // Two teams: each team's own pool.
    let mut state = State::default();
    state.set(Some(Arc::new(Context {
        lives: vec![Some(30), Some(1)],
        phase: Phase::Between,
        ..two_teams()
    })));
    let red = me_at([2.0, 0.0, 0.0], 0);
    assert_eq!(state.plan(&perception(red, vec![], true), &red, None), None);
    let blue = me_at([38.0, 0.0, 0.0], 1);
    assert_eq!(
        state.plan(&perception(blue, vec![], true), &blue, None),
        Some(Goal::Hold(AWAY, REGROUP_RADIUS))
    );
}

#[test]
fn bots_do_not_wander_to_a_drop_zone_alone() {
    // The drop zone at x = 14 is inside the errand radius (16) but
    // outside the hold (8).
    let mut state = State::default();
    state.set(Some(Arc::new(Context {
        phase: Phase::Between,
        ..one_team()
    })));
    let hold = Traits::of(None).hold;
    let me = me_at([12.0, 0.0, 0.0], 0);
    let alone = perception(me, vec![], true);
    assert_eq!(state.plan(&alone, &me, None), Some(Goal::Hold(HOME, hold)));
    // A gun lying at the drop zone is not worth the walk alone...
    assert!(state.blocks([14.0, 1.0, 0.0], &alone, &me, None));
    // ...but with a teammate along both are fine.
    let mut state = State::default();
    state.set(Some(Arc::new(Context {
        phase: Phase::Between,
        ..one_team()
    })));
    let company = perception(me, vec![seen(1, 0, [13.0, 1.0, 0.0])], true);
    assert_eq!(state.plan(&company, &me, None), None);
    assert!(!state.blocks([14.0, 1.0, 0.0], &company, &me, None));
    // A drop zone inside the hold radius is home ground.
    let mut state = State::default();
    state.set(Some(Arc::new(Context {
        phase: Phase::Between,
        drop_zones: vec![[hold - 1.0, 0.0, 0.0]],
        ..one_team()
    })));
    let near = me_at([hold - 2.0, 0.0, 0.0], 0);
    assert_eq!(state.plan(&perception(near, vec![], true), &near, None), None);
    // In the mind: between waves, a far enemy seen at a drop zone is not
    // chased out there; the bot goes home.
    let mut mind = Mind::new(Skill::Normal, 11);
    mind.set_firefight(Some(Arc::new(Context {
        phase: Phase::Between,
        ..one_team()
    })));
    let out = me_at([13.0, 0.0, 0.0], 0);
    let p = perception(out, vec![], true);
    mind.step(&p, None);
    assert_eq!(mind.doing, Doing::Play(Goal::Hold(HOME, hold)));
}

#[test]
fn actors_are_read_from_the_pool() {
    use guest_layout::datum::layout as dl;
    use guest_layout::handle::DatumHandle;
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::fake;

    const OBJECTS_HEADER: u32 = 0x0062_0000;
    const OBJECTS_DATA: u32 = 0x0062_1000;
    const BODIES: u32 = 0x0063_0000;
    let mut g = FakeGuest::new();
    // Three units: a living biped, a dead one, a living vehicle.
    g.put_u32(guest_layout::points::OBJECTS.base, OBJECTS_HEADER);
    g.map(OBJECTS_HEADER, 0x48)
        .put_u32(OBJECTS_HEADER + dl::ELEMENT_SIZE, 0xc)
        .put_u32(OBJECTS_HEADER + dl::HIGH_WATER, 3)
        .put_u32(OBJECTS_HEADER + dl::DATA, OBJECTS_DATA)
        .map(OBJECTS_DATA, 0xc * 3);
    let units: Vec<DatumHandle> = (0..3u32)
        .map(|i| {
            let unit = DatumHandle::new(0xb000 + i as u16, i);
            let (h, body) = (OBJECTS_DATA + i * 0xc, BODIES + i * 0x400);
            let kind = if i == 2 { objects::VEHICLE } else { objects::BIPED };
            g.put_u16(h, unit.salt()).put_u8(h + 3, kind).put_u32(h + 8, body);
            g.map(body, 0x400);
            for k in 0..3 {
                g.put_f32(body + 0x64 + 4 * k, 10.0 * i as f32 + k as f32);
            }
            let flags = if i == 1 { crate::guest::BODY_DEPLETED } else { 0 };
            g.put_u16(body + 0x10a, flags);
            unit
        })
        .collect();
    let pool = fake::datums::<Actor>(&mut g, 0x0064_0000, 8);
    g.put_u32(ACTORS.base, pool.array.addr);
    let mut ids = vec![];
    for unit in units.iter().map(|u| Some(u.raw())).chain([None]) {
        let (handle, a) = fake::push(&mut g, &pool);
        g.put_u32(a.addr + 0x18, unit.unwrap_or(u32::MAX))
            .put_u16(a.addr + 0x24, 3);
        ids.push(ACTOR_ID_BASE + u32::from(handle.index()));
    }
    let mut out = vec![seen(0, 0, HOME)];
    read_actors(&g, &mut out).unwrap();
    // The player already there is kept; the dead biped's actor and the
    // actor with no unit are left out.
    assert_eq!(
        out,
        vec![
            seen(0, 0, HOME),
            seen(ids[0], 3, [0.0, 1.0, 2.0]),
            seen(ids[2], 3, [20.0, 21.0, 22.0]),
        ]
    );
    assert!(out[1..].iter().all(|o| is_actor(o.id)));
    // No pool: none.
    g.put_u32(ACTORS.base, 0);
    let mut none = vec![];
    read_actors(&g, &mut none).unwrap();
    assert!(none.is_empty());
}

#[test]
fn a_clanker_never_targets_an_ally_actor() {
    // An ally squad's actors are on the players' engine team 0; the
    // enemies' are on the AI's team 3.
    let ctx = one_team();
    let mut p = perception(
        me_at(HOME, 0),
        vec![
            seen(ACTOR, 0, [5.0, 0.0, 0.0]),
            seen(ACTOR + 1, 3, [6.0, 0.0, 0.0]),
        ],
        true,
    );
    normalise(&mut p, &ctx);
    assert_eq!(targets(&p), vec![ACTOR + 1]);
    assert_eq!(crate::brain::target(&p), Some(0));
    assert_eq!(p.others.len(), 1);

    // Two player teams: a bot on team 1 drops team 1's ally and still
    // fights team 0's, which the engine makes an enemy.
    let two = Context {
        player_teams: vec![0, 1],
        bases: vec![HOME, [50.0, 0.0, 0.0]],
        lives: vec![Some(20), Some(20)],
        ..one_team()
    };
    let mut p = perception(
        me_at([50.0, 0.0, 0.0], 1),
        vec![
            seen(ACTOR, 1, [52.0, 0.0, 0.0]),
            seen(ACTOR + 1, 0, [53.0, 0.0, 0.0]),
        ],
        true,
    );
    normalise(&mut p, &two);
    assert_eq!(targets(&p), vec![ACTOR + 1]);

    // The AI on a player team (a misassignment) leaves no ally to tell.
    let bad = Context { ai_team: 0, ..one_team() };
    let mut p = perception(me_at(HOME, 0), vec![seen(ACTOR, 0, [5.0, 0.0, 0.0])], true);
    normalise(&mut p, &bad);
    assert_eq!(targets(&p), vec![ACTOR]);
}
