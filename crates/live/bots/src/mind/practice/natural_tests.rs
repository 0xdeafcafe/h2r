//! The practice played like a person (docs/game/bots.md, "How each bot
//! practises"): each name's own style, variety over the sites, fumbles,
//! letting players go first, chains and Monsters.

use super::style::{CHAIN_BANDS, MISS_BANDS, PATIENCE};
use super::*;
use crate::glitches::Bounce;
use crate::glitches::prime::PRIMED_WORD;
use crate::mind::Skill;
use crate::name_profile::NameProfile;
use crate::nav::{NavMesh, Poly};
use superbounce::{Shape, Status};

fn map() -> MapData {
    let floor = Poly::new(vec![
        [-40.0, -40.0, 0.0],
        [40.0, -40.0, 0.0],
        [40.0, 40.0, 0.0],
        [-40.0, 40.0, 0.0],
    ]);
    MapData {
        nav: NavMesh::new(vec![floor], vec![]),
        ..MapData::default()
    }
}

fn site(stand: Vec3) -> Site {
    Site {
        bounce: Bounce {
            map: "",
            name: "test",
            stand,
            spread: 0.03,
            drift: [1.0, 0.0],
            source: "scan",
            takeoff: None,
            landing: None,
        },
        launch: 12.3,
        status: Status::Likely,
        ..Site::default()
    }
}

fn me_at(position: Vec3) -> Me {
    Me {
        position,
        aim: [1.0, 0.0, 0.0],
        ..Me::default()
    }
}

fn seen(id: u32, position: Vec3) -> Seen {
    Seen {
        id,
        position,
        team: 1,
    }
}

fn looking(me: Me, others: Vec<Seen>, tick: u32) -> Perception {
    Perception {
        me: Some(me),
        others,
        teams: false,
        tick,
        ..Perception::default()
    }
}

const NAMES: [&str; 12] = [
    "Clanker 0",
    "Clanker 1",
    "Bouncer",
    "Xx Slide xX",
    "Lockout Larry",
    "Sniper Dave",
    "grenade gary",
    "Blue Team",
    "Mombasa",
    "Zanzibar",
    "Ascension",
    "Coagulation",
];

// ---- each name's own style ----

#[test]
fn each_name_practises_its_own_way_inside_its_skills_band() {
    for skill in [Skill::Easy, Skill::Normal, Skill::Heroic, Skill::Legendary] {
        let band = |b: [(f32, f32); 4]| b[skill as usize];
        let styles: Vec<Style> = NAMES
            .iter()
            .map(|n| Style::for_name(NameProfile::for_name(n).seed, skill))
            .collect();
        for s in &styles {
            let (lo, hi) = band(MISS_BANDS);
            assert!((lo..=hi).contains(&s.miss), "{skill:?} miss {}", s.miss);
            let (lo, hi) = band(CHAIN_BANDS);
            assert!((lo..=hi).contains(&s.chain), "{skill:?} chain {}", s.chain);
            assert!((PATIENCE.0..=PATIENCE.1).contains(&s.patience));
            assert!((0.0..=1.0).contains(&s.slots) && (0.0..=1.0).contains(&s.creases));
            assert!((0.4..=1.6).contains(&s.roam));
        }
        // A range, not a few settings: every name its own miss rate.
        let mut misses: Vec<f32> = styles.iter().map(|s| s.miss).collect();
        misses.sort_by(f32::total_cmp);
        misses.dedup();
        assert_eq!(misses.len(), NAMES.len(), "{skill:?}");
    }
    // The same name, the same style, whatever its case; another name, not.
    let a = Style::for_name(NameProfile::for_name("Bouncer").seed, Skill::Legendary);
    let b = Style::for_name(NameProfile::for_name("BOUNCER").seed, Skill::Legendary);
    let c = Style::for_name(NameProfile::for_name("Bouncer 2").seed, Skill::Legendary);
    assert_eq!(a, b);
    assert_ne!(a, c);
    // Legendary names fumble 3 to 10% of their tries.
    assert!((0.03..=0.10).contains(&a.miss));
}

#[test]
fn a_bot_practises_by_its_names_style_and_without_one_by_its_seed() {
    let mut named = Mind::new(Skill::Heroic, 5);
    named.name_profile = Some(NameProfile::for_name("Bouncer"));
    let want = Style::for_name(NameProfile::for_name("Bouncer").seed, Skill::Heroic);
    assert_eq!(named.practice_style(), want);
    // A new skill draws it again, in the new band.
    named.skill = Skill::Easy;
    assert_eq!(
        named.practice_style(),
        Style::for_name(NameProfile::for_name("Bouncer").seed, Skill::Easy)
    );
    let mut plain = Mind::new(Skill::Heroic, 5);
    assert_eq!(plain.practice_style(), Style::for_name(5, Skill::Heroic));
}

// ---- variety ----

#[test]
fn recent_tries_weigh_a_site_down_more_each_time_and_fade() {
    let k = stand_key([1.0, 2.0, 3.0]);
    let other = stand_key([4.0, 0.0, 0.0]);
    let mut list = VecDeque::new();
    assert_eq!(recency(&list, k, 1000), 0.0);
    push_recent(&mut list, k, 1000);
    push_recent(&mut list, other, 1000);
    let once = recency(&list, k, 1000);
    assert!((once - 1.0).abs() < 1e-6);
    push_recent(&mut list, k, 1000);
    assert!((recency(&list, k, 1000) - 2.0).abs() < 1e-6);
    // Fades with time: each try to about a third in RECENT_TICKS.
    let later = recency(&list, k, 1000 + RECENT_TICKS as u32);
    assert!(later < 0.8 && later > 0.6, "{later}");
    // Kept to RECENT_KEEP tries.
    for t in 0..2 * RECENT_KEEP as u32 {
        push_recent(&mut list, other, t);
    }
    assert_eq!(list.len(), RECENT_KEEP);
    assert_eq!(recency(&list, k, 1000), 0.0);
    // The more recent tries, the less it weighs.
    let s = site([10.0, 0.0, 0.0]);
    let st = Style::default();
    let w = |recent| weight(&s, [0.0; 3], &Weigh { recent, ..Weigh::default() }, &st);
    assert!(w(0.0) > w(1.0) && w(1.0) > w(2.0) && w(2.0) > w(3.0));
}

/// The site a pick lands on, picking again while it pairs.
fn pick(mind: &mut Mind, at: Vec3, tick: u32, m: &MapData) -> (Site, Option<Crouch>) {
    for t in tick..tick + 20 {
        match mind.pick_practice(at, t, Some(m)) {
            Pick::Site(s, c) => return (s, c),
            Pick::Later => {}
            Pick::Nothing => panic!("nothing to pick"),
        }
    }
    panic!("no pick")
}

#[test]
fn a_bot_goes_round_the_sites_rather_than_cycling_a_few() {
    let m = map();
    // Six sites the same way off, round the bot: only the practice's own
    // memory and the random part tell them apart.
    let sites: Vec<Site> = (0..6)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 6.0;
            site([15.0 * a.cos(), 15.0 * a.sin(), 0.0])
        })
        .collect();
    for seed in [71, 72, 73] {
        let mut mind = Mind::new(Skill::Legendary, seed);
        static BOARD: Board = Mutex::new(Vec::new());
        mind.practice.board = &BOARD;
        mind.known = Known {
            sites: sites.clone(),
            crouches: vec![Crouch::at([0.0, 1.0, 0.0])],
        };
        let mut seq = Vec::new();
        let mut tick = 1000;
        for _ in 0..18 {
            let (s, c) = pick(&mut mind, [0.0; 3], tick, &m);
            mind.practice.remember(s.bounce.stand, c, tick);
            seq.push(sites.iter().position(|x| *x == s).unwrap());
            // About a try's length apart.
            tick += 450;
        }
        let mut seen: Vec<usize> = seq.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 6, "every site: {seq:?}");
        assert!(seq.windows(2).all(|w| w[0] != w[1]), "never twice running: {seq:?}");
        for i in 0..6 {
            assert!(seq.iter().filter(|&&x| x == i).count() <= 5, "{seq:?}");
        }
    }
}

#[test]
fn other_bots_crowd_a_site_and_its_neighbours_smoothly() {
    static BOARD: Board = Mutex::new(Vec::new());
    let tick = 4000;
    let at = [0.0, 0.0, 0.0];
    claim(&BOARD, 0xc1, Some(at), tick);
    assert_eq!(crowd(&BOARD, 0xc0, at, tick), 1.0);
    // Its own claim does not crowd it.
    assert_eq!(crowd(&BOARD, 0xc1, at, tick), 0.0);
    let near = crowd(&BOARD, 0xc0, [2.0, 0.0, 0.0], tick);
    let farther = crowd(&BOARD, 0xc0, [4.0, 0.0, 0.0], tick);
    assert!(near > farther && farther > 0.0 && near <= CROWD_NEAR, "{near} {farther}");
    assert_eq!(crowd(&BOARD, 0xc0, [CROWD_RADIUS + 1.0, 0.0, 0.0], tick), 0.0);
    // Lapsed claims crowd nothing.
    assert_eq!(crowd(&BOARD, 0xc0, at, tick + CLAIM_TICKS), 0.0);
}

#[test]
fn a_site_it_goes_again_at_is_picked_though_just_tried_and_resting() {
    let m = map();
    let a = site([10.0, 0.0, 0.0]);
    let b = site([-10.0, 0.0, 0.0]);
    let mut mind = Mind::new(Skill::Normal, 74);
    static BOARD: Board = Mutex::new(Vec::new());
    mind.practice.board = &BOARD;
    mind.known = Known {
        sites: vec![a, b],
        crouches: vec![Crouch::at([0.0, 1.0, 0.0])],
    };
    let tick = 2000;
    mind.practice.remember(a.bounce.stand, None, tick);
    mind.stuck.blacklist(a.bounce.stand, tick);
    assert_eq!(pick(&mut mind, [0.0; 3], tick, &m).0, b, "resting");
    mind.practice.again = Some(a);
    assert_eq!(pick(&mut mind, [0.0; 3], tick, &m).0, a, "again");
}

// ---- Monsters ----

#[test]
fn monster_sites_are_those_with_lines_converging_or_a_monsters_launch() {
    let base = site([0.0; 3]);
    let with = |shape, score, launch| Site {
        shape,
        score,
        launch,
        ..base
    };
    assert!(!with(Shape::Slot, 0.7, 12.3).monster());
    assert!(with(Shape::Slot, 0.85, 12.3).monster());
    assert!(!with(Shape::Crease, 0.5, 5.2).monster());
    assert!(with(Shape::Crease, 0.9, 5.2).monster());
    assert!(with(Shape::Vee, 0.6, 5.2).monster());
    assert!(with(Shape::Busy, 0.2, 3.0).monster());
    assert!(with(Shape::Slot, 0.0, 16.5).monster(), "a verified Monster launch");
    // A measured site is a slot no scan scored.
    assert!(!Site::measured(base.bounce).monster());
}

#[test]
fn a_monster_loving_name_weighs_monster_sites_up_and_tastes_tell_shapes_apart() {
    let plain = site([10.0, 0.0, 0.0]);
    let monster = Site {
        shape: Shape::Slot,
        score: 0.9,
        ..plain
    };
    let keen = Style {
        monster: 0.95,
        ..Style::default()
    };
    let cool = Style {
        monster: 0.1,
        ..Style::default()
    };
    let w = |s: &Site, st: &Style| weight(s, [0.0; 3], &Weigh::default(), st);
    let pull = |st: &Style| w(&monster, st) - w(&plain, st);
    assert!((pull(&keen) - W_MONSTER * 0.95).abs() < 1e-5);
    assert!(pull(&keen) > pull(&cool) && pull(&cool) > 0.0);
    // A crease lover weighs a crease up, a slot lover a slot.
    let crease = Site {
        shape: Shape::Crease,
        ..plain
    };
    let creasey = Style {
        slots: 0.1,
        creases: 0.9,
        ..Style::default()
    };
    assert!(w(&crease, &creasey) > w(&plain, &creasey));
    let slotty = Style {
        slots: 0.9,
        creases: 0.1,
        ..Style::default()
    };
    assert!(w(&plain, &slotty) > w(&crease, &slotty));
    // A wanderer minds the distance less than a homebody.
    let far = site([90.0, 0.0, 0.0]);
    let roam = |r: f32| {
        let st = Style {
            roam: r,
            ..Style::default()
        };
        w(&plain, &st) - w(&far, &st)
    };
    assert!(roam(0.4) < roam(1.6));
}

// ---- fumbles ----

fn draws(seed: u64) -> impl FnMut() -> f32 {
    let mut r = Rng::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    move || r.unit()
}

#[test]
fn a_fumbled_standing_jump_lands_a_little_off_the_line() {
    let s = site([10.0, 0.0, 0.0]);
    for seed in 1..40 {
        let (f, how) = fumble(s, false, false, &mut draws(seed));
        assert_eq!(how, "off_line");
        assert_eq!(f.bounce.stand, s.bounce.stand, "the same site");
        let [x, y] = f.knobs.aim.expect("an aim");
        // Across the seam (along its drift), past the 0.1 a bounce needs.
        assert_eq!(y, 0.0);
        let off = (x - 10.0).abs();
        assert!((FUMBLE_OFF.0..=FUMBLE_OFF.1 + 1e-5).contains(&off), "{off}");
    }
}

#[test]
fn a_fumbled_drop_goes_wrong_in_the_ways_a_drop_can() {
    let s = site([10.0, 0.0, 0.0]);
    let mut kinds = std::collections::BTreeSet::new();
    for seed in 1..200 {
        let (f, how) = fumble(s, true, true, &mut draws(seed));
        kinds.insert(how);
        let early = f.knobs.early.unwrap_or(TAKEOFF_EARLY);
        match how {
            "early" => assert!(early >= TAKEOFF_EARLY + FUMBLE_EARLY.0 - 1e-5),
            "late" => assert!(early <= TAKEOFF_EARLY - FUMBLE_LATE.0 + 1e-5),
            "lazy_air" => {
                let l = f.knobs.letgo.unwrap();
                assert!((FUMBLE_LETGO.0..=FUMBLE_LETGO.1 + 1e-5).contains(&l));
            }
            "off_line" => assert!(f.knobs.aim.is_some()),
            other => panic!("{other}"),
        }
    }
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    // A run off the edge has no takeoff to mistime.
    for seed in 1..100 {
        let (_, how) = fumble(s, true, false, &mut draws(seed));
        assert!(matches!(how, "off_line" | "lazy_air"), "{how}");
    }
    // A busy point (no drift) is missed any way round.
    let busy = Site {
        bounce: Bounce {
            drift: [0.0, 0.0],
            ..s.bounce
        },
        ..s
    };
    let (f, _) = fumble(busy, false, false, &mut draws(3));
    let [x, y] = f.knobs.aim.unwrap();
    let off = (x - 10.0).hypot(y);
    assert!((FUMBLE_OFF.0 - 1e-5..=FUMBLE_OFF.1 + 1e-5).contains(&off));
}

// ---- chains ----

#[test]
fn a_chain_link_is_one_of_the_easiest_and_mostly_another_site() {
    let a = site([0.0; 3]);
    let b = site([1.0, 0.0, 0.0]);
    let c = site([2.0, 0.0, 0.0]);
    let leg = Leg {
        time: 5.0,
        steer: 0.0,
        speed_in: 12.0,
    };
    let near = [(a, leg), (b, leg), (c, leg)];
    // Never keen: no chain.
    for seed in 1..20 {
        assert_eq!(chain_pick(&a, &near, 0.0, &mut draws(seed)), None);
    }
    // Always keen: always a link, mostly not straight back onto a.
    let mut same = 0;
    for seed in 1..400 {
        let i = chain_pick(&a, &near, 1.0, &mut draws(seed)).expect("a link");
        if i == 0 {
            same += 1;
        }
    }
    assert!(same > 0 && same < 200, "{same}");
    // Only itself in reach: back onto it.
    assert_eq!(chain_pick(&a, &near[..1], 1.0, &mut draws(9)), Some(0));
    assert_eq!(chain_pick(&a, &[], 1.0, &mut draws(9)), None);
}

// ---- letting players go first ----

#[test]
fn a_floor_is_in_use_by_a_player_on_it_or_on_the_way_onto_it() {
    let z = Zone::site([9.2, 0.0, 0.0], [10.0, 0.0, 0.0]);
    let still = |at: Vec3| Other {
        id: 1,
        at,
        vel: [0.0; 3],
        tick: 0,
    };
    assert_eq!(use_of(&z, &still([9.6, 0.5, 0.0])), 1.0);
    // The landing runs on past the target.
    assert_eq!(use_of(&z, &still([10.8, 0.0, 0.0])), 1.0);
    assert_eq!(use_of(&z, &still([6.0, 0.0, 0.0])), 0.0);
    // Another floor over or under it is not it; a flight over it is.
    assert_eq!(use_of(&z, &still([9.6, 0.0, 6.0])), 0.0);
    let flying = Other {
        vel: [0.0, 0.0, -5.0],
        ..still([9.6, 0.0, 6.0])
    };
    assert_eq!(use_of(&z, &flying), 1.0);
    // Running at it from 3 units off: on the way.
    let coming = Other {
        vel: [2.0, 0.0, 0.0],
        ..still([6.5, 0.0, 0.0])
    };
    assert_eq!(use_of(&z, &coming), yielding::APPROACHING);
    // Running away from it: not.
    let going = Other {
        vel: [-2.0, 0.0, 0.0],
        ..still([6.5, 0.0, 0.0])
    };
    assert_eq!(use_of(&z, &going), 0.0);
}

#[test]
fn a_standing_bot_is_in_a_moving_players_way_only_ahead_of_them() {
    let runner = Other {
        id: 2,
        at: [0.0, 0.0, 0.0],
        vel: [2.0, 0.0, 0.0],
        tick: 0,
    };
    // Ahead, just left of the course: step further left.
    assert_eq!(in_the_way([2.0, 0.3, 0.0], &runner), Some([0.0, 1.0]));
    assert_eq!(in_the_way([2.0, -0.3, 0.0], &runner), Some([0.0, -1.0]));
    // Behind, well off the course, too far ahead, or the player standing.
    assert_eq!(in_the_way([-1.0, 0.0, 0.0], &runner), None);
    assert_eq!(in_the_way([2.0, 2.0, 0.0], &runner), None);
    assert_eq!(in_the_way([9.0, 0.0, 0.0], &runner), None);
    let still = Other {
        vel: [0.0; 3],
        ..runner
    };
    assert_eq!(in_the_way([1.0, 0.0, 0.0], &still), None);
}

#[test]
fn the_tracker_gives_each_player_its_speed_from_the_last_sample() {
    let mut o = yielding::Others::default();
    o.see(&[seen(3, [0.0, 0.0, 0.0])], 10);
    assert_eq!(o.now[0].vel, [0.0; 3]);
    o.see(&[seen(3, [0.1, 0.0, 0.0]), seen(4, [5.0, 0.0, 0.0])], 11);
    assert!((o.now[0].vel[0] - 3.0).abs() < 1e-4);
    assert_eq!(o.now[1].vel, [0.0; 3]);
    // A gap too long gives no speed; a player gone is gone.
    o.see(&[seen(3, [1.0, 0.0, 0.0])], 20);
    assert_eq!((o.now.len(), o.now[0].vel), (1, [0.0; 3]));
}

#[test]
fn bots_waiting_for_each_other_let_the_first_claim_go_first() {
    static BOARD: Board = Mutex::new(Vec::new());
    let tick = 6000;
    // Bot A (slot 1) claimed at 5900, bot B (slot 2) at 5950.
    claim_as(&BOARD, 0xa, Some([1.0, 0.0, 0.0]), 5900, Some(1), false);
    claim_as(&BOARD, 0xb, Some([2.0, 0.0, 0.0]), 5950, Some(2), false);
    // Renewed: the claim keeps when it began.
    claim_as(&BOARD, 0xa, Some([1.0, 0.0, 0.0]), tick, Some(1), true);
    claim_as(&BOARD, 0xb, Some([2.0, 0.0, 0.0]), tick, Some(2), true);
    let a = claim_of(&BOARD, 0xa, tick).unwrap();
    let b = claim_of(&BOARD, 0xb, tick).unwrap();
    assert_eq!((a.since, b.since), (5900, 5950));
    let player = |id| Other {
        id,
        at: [0.0; 3],
        vel: [0.0; 3],
        tick,
    };
    // Both waiting: B lets A go, A does not wait for B.
    assert!(defers(&BOARD, Some(b), &player(1), tick));
    assert!(!defers(&BOARD, Some(a), &player(2), tick));
    // A bot on its try (not waiting) and a person always go first.
    claim_as(&BOARD, 0xb, Some([2.0, 0.0, 0.0]), tick, Some(2), false);
    assert!(defers(&BOARD, Some(a), &player(2), tick));
    assert!(defers(&BOARD, Some(a), &player(9), tick));
    // A claim on another site begins again.
    claim_as(&BOARD, 0xa, Some([3.0, 0.0, 0.0]), tick + 1, Some(1), false);
    assert_eq!(claim_of(&BOARD, 0xa, tick + 1).unwrap().since, tick + 1);
}

/// A practising bot at `at`, primed (its route goes straight to the
/// approach), with the one site at (10, 0, 0) and its own claims board.
fn primed_bot(seed: u64) -> Mind {
    let mut mind = Mind::new(Skill::Legendary, seed);
    let board: &'static Board = Box::leak(Box::default());
    mind.practice.board = board;
    mind.known = Known {
        sites: vec![site([10.0, 0.0, 0.0])],
        crouches: vec![Crouch::at([3.0, 0.0, 0.0])],
    };
    mind.pair_word = Some(PRIMED_WORD);
    mind.set_practice(true);
    mind
}

#[test]
fn a_bot_lets_a_player_on_its_site_go_first_then_goes_on() {
    let m = map();
    let mut mind = primed_bot(81);
    let me = me_at([7.0, 0.0, 0.0]);
    let player = seen(7, [9.6, 0.3, 0.0]);
    let mut events = Vec::new();
    let mut t = 0;
    while mind.practice_view().unwrap().state != "yielding" && t < 300 {
        mind.step(&looking(me, vec![player], t), Some(&m));
        events.extend(mind.take_practice_events());
        t += 1;
    }
    assert_eq!(mind.practice_view().unwrap().state, "yielding");
    assert_eq!(mind.practice_view().unwrap().site, Some([10.0, 0.0, 0.0]));
    // The route was dropped before the jump: no try, no strike, no rest.
    assert_eq!(mind.order_view().error, Some("yield"));
    assert!(!events.iter().any(|e| matches!(e, PracticeEvent::Tried { .. })));
    assert!(!mind.stuck.blocked([10.0, 0.0, 0.0], t));
    // It watches the player while they stay.
    for _ in 0..60 {
        let pad = mind.step(&looking(me, vec![player], t), Some(&m));
        assert!(!pad.buttons.contains(crate::mind::JUMP), "no jump while waiting");
        events.extend(mind.take_practice_events());
        t += 1;
    }
    assert_eq!(mind.practice_view().unwrap().state, "yielding");
    // The player is done and gone: a moment later the try goes on.
    for _ in 0..YIELD_CLEAR + 5 {
        mind.step(&looking(me, vec![], t), Some(&m));
        events.extend(mind.take_practice_events());
        t += 1;
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PracticeEvent::Yielded { gave_up: false, waited, .. } if *waited >= 60)),
        "{events:?}"
    );
    assert_eq!(mind.practice_view().unwrap().state, "route");
}

#[test]
fn a_bot_whose_patience_runs_out_tries_elsewhere() {
    let m = map();
    let mut mind = primed_bot(82);
    let patience = mind.practice_style().patience;
    let me = me_at([7.0, 0.0, 0.0]);
    let player = seen(7, [9.6, 0.3, 0.0]);
    let mut events = Vec::new();
    let mut at = 0;
    for t in 0..PATIENCE.1 + 200 {
        at = t;
        mind.step(&looking(me, vec![player], t), Some(&m));
        events.extend(mind.take_practice_events());
        if events
            .iter()
            .any(|e| matches!(e, PracticeEvent::Yielded { gave_up: true, .. }))
        {
            break;
        }
    }
    let waited = events.iter().find_map(|e| match e {
        PracticeEvent::Yielded {
            gave_up: true,
            waited,
            ..
        } => Some(*waited),
        _ => None,
    });
    assert_eq!(waited, Some(patience + 1), "{events:?}");
    // Counted as tried: the site weighs less now.
    assert!(recency(&mind.practice.recent, stand_key([10.0, 0.0, 0.0]), at) > 0.9);
    assert_eq!(mind.practice_view().unwrap().state, "idle");
}

#[test]
fn a_bot_far_from_its_site_walks_on_though_a_player_is_there() {
    let m = map();
    let mut mind = primed_bot(83);
    // 9 units back: past YIELD_BACK of the floor's reach.
    let me = me_at([0.2, 0.0, 0.0]);
    let player = seen(7, [9.6, 0.3, 0.0]);
    let mut t = 0;
    while mind.practice_view().unwrap().state != "route" && t < 120 {
        mind.step(&looking(me, vec![player], t), Some(&m));
        t += 1;
    }
    for t in t..t + 10 {
        mind.step(&looking(me, vec![player], t), Some(&m));
    }
    assert_eq!(mind.practice_view().unwrap().state, "route");
    assert_eq!(mind.order_view().state, "running");
}

#[test]
fn a_bot_standing_between_tries_steps_out_of_a_runners_way() {
    let m = map();
    let mut mind = Mind::new(Skill::Normal, 84);
    let board: &'static Board = Box::leak(Box::default());
    mind.practice.board = board;
    mind.known = Known {
        sites: vec![site([30.0, 30.0, 0.0])],
        crouches: vec![Crouch::at([28.0, 28.0, 0.0])],
    };
    mind.set_practice(true);
    let me = me_at([0.0, 0.0, 0.0]);
    // Standing (the first look round), then a player runs at it.
    mind.step(&looking(me, vec![], 0), Some(&m));
    let mut events = Vec::new();
    for t in 1..6 {
        let x = 3.0 - 0.07 * t as f32;
        mind.step(&looking(me, vec![seen(7, [x, 0.2, 0.0])], t), Some(&m));
        events.extend(mind.take_practice_events());
    }
    assert!(
        events.iter().any(|e| matches!(e, PracticeEvent::SteppedAside { .. })),
        "{events:?}"
    );
    // A plain walk off the course, not a route round the player.
    let v = mind.order_view();
    assert_eq!((v.state, v.steps.as_slice()), ("running", ["nav"].as_slice()));
}
