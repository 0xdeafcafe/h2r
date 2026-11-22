use super::*;
use crate::glitches::SPOTS;

pub(crate) fn bounce_spot(stand: Vec3, launch: f32, score: f32) -> Spot {
    Spot {
        kind: Kind::Bounce,
        edge: [stand, [stand[0], stand[1] + 0.3, stand[2]]],
        stand,
        fall: 0.0,
        launch,
        drift: [0.0, 1.0],
        score,
        need: 4.3,
        ..Spot::default()
    }
}

pub(crate) fn crouch_spot(stand: Vec3) -> Spot {
    Spot {
        kind: Kind::Crouch,
        edge: [stand, [stand[0] + 1.0, stand[1], stand[2]]],
        stand,
        fall: 0.0,
        launch: 0.0,
        drift: [0.0, 0.0],
        score: 0.9,
        out: [0.0, 1.0],
        lip: superbounce::Lip::new(&[[stand, [stand[0] + 0.4, stand[1], stand[2] + 0.5]]]),
        ..Spot::default()
    }
}

#[test]
fn a_scanned_bounce_becomes_a_bounce_with_the_scan_as_its_source() {
    let s = Spot {
        drift: [1.0, 0.0],
        fall: 0.2,
        ..bounce_spot([4.0, 5.0, 1.0], 14.0, 0.8)
    };
    let site = Site::from_spot(&s).unwrap();
    assert_eq!(site.bounce.stand, [4.0, 5.0, 1.0]);
    assert_eq!(site.bounce.drift, [1.0, 0.0]);
    assert_eq!(site.bounce.source, "scan");
    assert_eq!(site.bounce.spread, SCAN_SPREAD);
    assert_eq!(site.launch, 14.0);
    assert_eq!(site.fall, 0.2);
    assert!(site.standing());
    // The walk to it ends behind it, against its drift.
    assert_eq!(site.bounce.from(), [4.0 - crate::glitches::spots::APPROACH, 5.0, 1.0]);
    assert!(Site::from_spot(&crouch_spot([0.0; 3])).is_none());
}

#[test]
fn a_drop_onto_the_slot_is_not_a_standing_site() {
    let s = Spot {
        fall: 3.0,
        ..bounce_spot([0.0; 3], 14.0, 0.8)
    };
    assert!(!Site::from_spot(&s).unwrap().standing());
}

#[test]
fn measured_spots_come_first_and_scans_follow_best_first() {
    let measured = SPOTS[0];
    let scan = [
        bounce_spot([50.0, 0.0, 0.0], 10.0, 0.5),
        // The measured spot again, scanned.
        bounce_spot([measured.stand[0] + 0.2, measured.stand[1], measured.stand[2]], 13.0, 0.99),
        bounce_spot([60.0, 0.0, 0.0], 15.0, 0.9),
        // Too unsure.
        bounce_spot([70.0, 0.0, 0.0], 15.0, 0.1),
        crouch_spot([5.0, 5.0, 0.0]),
    ];
    let k = Known::merge(vec![measured], &scan);
    let stands: Vec<_> = k.sites.iter().map(|s| s.bounce.stand[0]).collect();
    assert_eq!(stands, vec![measured.stand[0], 60.0, 50.0]);
    assert_eq!(k.sites[0].bounce.source, measured.source);
    assert_eq!(k.sites[0].launch, MEASURED_LAUNCH);
    assert_eq!(k.crouches.len(), 1);
    assert_eq!((k.crouches[0].stand, k.crouches[0].out), ([5.0, 5.0, 0.0], [0.0, 1.0]));
    assert!(k.sites[0].proven() && !k.sites[1].proven());
}

#[test]
fn the_nearest_crouch_spot_is_by_straight_line() {
    let k = Known {
        sites: Vec::new(),
        crouches: vec![Crouch::at([10.0, 0.0, 0.0]), Crouch::at([2.0, 2.0, 0.0])],
    };
    assert_eq!(k.nearest_crouch([0.0, 0.0, 0.0]), Some(Crouch::at([2.0, 2.0, 0.0])));
    assert_eq!(Known::default().nearest_crouch([0.0; 3]), None);
}

#[test]
fn the_spots_file_round_trips_and_refuses_damage() {
    let proven = Spot { status: superbounce::Status::Proven, ..crouch_spot([7.0, 8.0, 9.0]) };
    let failed = Spot { status: superbounce::Status::Failed, ..bounce_spot([7.0, 8.0, 9.0], 9.0, 0.5) };
    let crease = Spot { shape: superbounce::Shape::Crease, ..bounce_spot([2.0, 3.0, 4.0], 5.2, 0.5) };
    let vee = Spot { shape: superbounce::Shape::Vee, ..bounce_spot([2.0, 9.0, 4.0], 26.0, 0.95) };
    let busy = Spot {
        shape: superbounce::Shape::Busy,
        edge: [[2.5, 9.5, 4.0]; 2],
        drift: [0.0; 2],
        ..bounce_spot([2.5, 9.5, 4.01], 3.0, 0.3)
    };
    let spots = vec![bounce_spot([1.0, 2.0, 3.0], 12.5, 0.7), crouch_spot([4.0, 5.0, 6.0]), proven, failed, crease, vee, busy];
    let stamp = Stamp::now(0xfeed);
    let b = to_bytes(&spots, stamp);
    assert_eq!(from_bytes(&b), Some((spots, stamp)));
    assert_eq!(from_bytes(&b[..b.len() - 1]), None);
    assert_eq!(from_bytes(b"H2SP"), None);
    assert_eq!(from_bytes(&[]), None);
    let mut bad = b.clone();
    bad[HEADER] = 9;
    assert_eq!(from_bytes(&bad), None);
}

#[test]
fn the_file_is_read_only_under_its_own_stamp() {
    let dir = std::env::temp_dir().join(format!("h2-spots-{}", std::process::id()));
    let spots = vec![crouch_spot([1.0, 1.0, 1.0])];
    write(&dir, "ascension", Stamp::now(7), &spots).unwrap();
    assert_eq!(read(&dir, "ascension", Stamp::now(7)), Some(spots));
    assert_eq!(read(&dir, "ascension", Stamp::now(8)), None);
    assert_eq!(read(&dir, "lockout", Stamp::now(7)), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_published_scan_is_found_by_map_in_any_case_and_bumps_the_generation() {
    let before = generation();
    assert!(scanned("Spots_Test_Map").is_empty());
    publish("spots_test_map", vec![crouch_spot([0.0; 3])]);
    assert!(generation() > before);
    assert_eq!(scanned("SPOTS_TEST_MAP").len(), 1);
    assert_eq!(Known::for_map("spots_test_map").crouches.len(), 1);
}

#[test]
fn the_listing_has_the_measured_spots_first_then_the_scan_best_first() {
    let map = "listing_test_map";
    assert!(!has_scan(map));
    publish(map, vec![bounce_spot([1.0; 3], 10.0, 0.4), crouch_spot([2.0; 3]), bounce_spot([3.0; 3], 11.0, 0.8)]);
    assert!(has_scan(map));
    let l = listing(map);
    let scores: Vec<_> = l.iter().map(|l| (l.kind, l.source, l.spot.score)).collect();
    assert_eq!(
        scores,
        vec![("crouch", "scan", 0.9), ("bounce", "scan", 0.8), ("bounce", "scan", 0.4)]
    );
    // Ascension has a measured spot, listed first and whatever the scan.
    let l = listing("ascension");
    assert_eq!((l[0].kind, l[0].source, l[0].spot.score), ("bounce", "measured", 1.0));
    assert_eq!(l[0].spot.stand, SPOTS[0].stand);
}

fn entry(kind: Kind, status: Status, at: Vec3) -> Entry {
    Entry {
        kind,
        status,
        at,
        trials: 4,
        ok: u32::from(status == Status::Proven),
        ..Entry::default()
    }
}

#[test]
fn verified_entries_prefer_what_worked_and_drop_what_failed() {
    let measured = SPOTS[0];
    let scan = [
        bounce_spot([50.0, 0.0, 0.0], 10.0, 0.9),
        bounce_spot([60.0, 0.0, 0.0], 15.0, 0.5),
        bounce_spot([70.0, 0.0, 0.0], 15.0, 0.6),
        crouch_spot([5.0, 5.0, 0.0]),
        crouch_spot([9.0, 9.0, 0.0]),
    ];
    let ver = [
        Entry {
            offset: [0.0, 0.05],
            height: Some(2.4),
            best: 11.5,
            ..entry(Kind::Bounce, Status::Proven, [60.0, 0.0, 0.0])
        },
        entry(Kind::Bounce, Status::Failed, [50.1, 0.0, 0.0]),
        Entry {
            offset: [0.02, -0.01],
            yaw: Some(-121.0),
            ..entry(Kind::Crouch, Status::Proven, [9.0, 9.0, 0.0])
        },
        entry(Kind::Crouch, Status::Failed, [5.0, 5.0, 0.0]),
        // The measured spot, failed by a game run.
        entry(Kind::Bounce, Status::Failed, measured.stand),
    ];
    let k = Known::merge_verified(vec![measured], &scan, &ver);
    let stands: Vec<_> = k.sites.iter().map(|s| s.bounce.stand[0]).collect();
    // The measured one failed; the proven scan first, then the rest best
    // first; the failed scan gone.
    assert_eq!(stands, vec![60.0, 70.0]);
    let p = k.sites[0];
    assert!(p.proven() && !p.standing());
    assert_eq!(p.target(), [60.0, 0.05, 0.0]);
    assert_eq!((p.drop_height(), p.launch), (2.4, 11.5));
    assert_eq!(k.crouches.len(), 1);
    assert_eq!(k.crouches[0].stand, [9.0, 9.0, 0.0]);
    assert_eq!(k.crouches[0].start, Some(([0.02, -0.01], Some(-121.0))));
    // With no verified file the measured spot is proven and first.
    let k = Known::merge(vec![measured], &scan);
    assert!(k.sites[0].proven() && k.sites[0].bounce.stand == measured.stand);
}

#[test]
fn a_measured_spot_keeps_its_traced_landing_when_a_drop_on_it_is_verified() {
    // The drill's first bounce wrote the touchdown it saw, a tick before
    // contact, as the working point: (-10.28, 3.91). The aim stays the
    // middle of the traced bounces.
    let measured = SPOTS[0];
    let ver = [Entry {
        offset: [0.34, 0.03],
        height: Some(2.43),
        best: 13.1,
        ..entry(Kind::Bounce, Status::Proven, measured.stand)
    }];
    let k = Known::merge_verified(vec![measured], &[], &ver);
    let s = k.sites[0];
    assert!(s.proven());
    assert_eq!(Some(s.target()), measured.landing);
    assert_eq!((s.drop_height(), s.launch), (2.43, 13.1));
}

#[test]
fn a_verified_file_is_stored_merged_read_back_and_bumps_the_generation() {
    let dir = std::env::temp_dir().join(format!("h2-verified-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    set_verified_dir(dir.clone());
    let map = "verified_test_map";
    assert!(verified_for(map).is_empty());
    let before = generation();
    let path = store_verified(map, &[entry(Kind::Crouch, Status::Failed, [1.0, 2.0, 3.0])])
        .unwrap()
        .unwrap();
    assert!(generation() > before);
    assert!(path.ends_with(verified_file_name(map)));
    store_verified(map, &[
        entry(Kind::Crouch, Status::Proven, [1.1, 2.0, 3.0]),
        entry(Kind::Bounce, Status::Failed, [4.0, 5.0, 6.0]),
    ])
    .unwrap();
    let on_disk = read_verified(&dir, map).unwrap();
    assert_eq!(on_disk.len(), 2);
    assert_eq!(on_disk[0].status, Status::Proven);
    assert_eq!(verified_for("VERIFIED_TEST_MAP").len(), 2);
    // The listing carries the statuses.
    publish(map, vec![crouch_spot([1.0, 2.0, 3.0]), bounce_spot([4.0, 5.0, 6.0], 10.0, 0.5)]);
    let l = listing(map);
    let st: Vec<_> = l.iter().map(|l| l.spot.status).collect();
    assert_eq!(st, vec![Status::Proven, Status::Failed]);
    assert!(Known::for_map(map).sites.is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- the learned file (docs/game/bounce-search.md) ----

fn line(state: learned::State, at: Vec3) -> learned::Entry {
    learned::Entry {
        state,
        at,
        ..learned::Entry::default()
    }
}

#[test]
fn a_dead_site_is_left_out_unless_proven() {
    use learned::State as L;
    let measured = SPOTS[0];
    let scan = [
        bounce_spot([50.0, 0.0, 0.0], 10.0, 0.9),
        bounce_spot([60.0, 0.0, 0.0], 15.0, 0.5),
        bounce_spot([70.0, 0.0, 0.0], 15.0, 0.6),
        bounce_spot([80.0, 0.0, 0.0], 15.0, 0.4),
    ];
    // 60 is proven by a verified entry.
    let ver = [entry(Kind::Bounce, Status::Proven, [60.0, 0.0, 0.0])];
    let aim = Knobs {
        how: Some(learned::How::Stand),
        aim: Some([70.1, 0.05]),
        ..Knobs::default()
    };
    let lines = [
        line(L::Dead, [50.0, 0.0, 0.0]),
        line(L::Dead, [60.0, 0.0, 0.0]),
        learned::Entry {
            knobs: aim,
            ..line(L::Learned, [70.0, 0.0, 0.0])
        },
        line(L::Blocked, [80.0, 0.0, 0.0]),
        // The measured spot is proven by a game run: dead or not, kept.
        line(L::Dead, measured.stand),
    ];
    let k = Known::merge_verified(vec![measured], &scan, &ver).with_learned(&lines);
    let stands: Vec<_> = k.sites.iter().map(|s| s.bounce.stand[0]).collect();
    // The dead likely one gone; the proven dead one, the measured one, the
    // learned one and the blocked one kept.
    assert_eq!(stands, vec![measured.stand[0], 60.0, 70.0, 80.0]);
    // A dead line's knobs are not played; a learned line's are.
    let s60 = k.sites[1];
    assert!(s60.knobs.is_default() && s60.line == Some(L::Dead));
    let s70 = k.sites[2];
    assert_eq!((s70.knobs, s70.line), (aim, Some(L::Learned)));
    assert_eq!(s70.target(), [70.1, 0.05, 0.0]);
    assert_eq!(s70.own_target(), [70.0, 0.0, 0.0]);
    assert!(s70.standing());
    // A blocked line changes nothing.
    let s80 = k.sites[3];
    assert!(s80.knobs.is_default() && s80.target() == [80.0, 0.0, 0.0]);
    // With no learned file, every site as before.
    let plain = Known::merge_verified(vec![measured], &scan, &ver);
    assert_eq!(plain.clone().with_learned(&[]), plain);
    assert_eq!(plain.sites.len(), 5);
}

#[test]
fn learned_lines_are_written_in_place_of_their_sites_and_read_back() {
    use learned::State as L;
    let dir = std::env::temp_dir().join(format!("h2-learned-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let map = "Learned_Test_Map";
    assert!(read_learned(&dir, map).unwrap().is_empty());
    let a = learned::Entry {
        tries: 3,
        ..line(L::Open, [1.0, 2.0, 3.0])
    };
    let b = line(L::Open, [4.0, 5.0, 6.0]);
    let (path, all) = write_learned(&dir, map, &[a.clone(), b.clone()]).unwrap();
    assert!(path.ends_with(learned_file_name("learned_test_map")));
    assert_eq!(all.len(), 2);
    // The same site further on replaces its line; the other is kept.
    let a2 = learned::Entry {
        tries: 9,
        hits: 2,
        ..a.clone()
    };
    write_learned(&dir, map, std::slice::from_ref(&a2)).unwrap();
    let back = read_learned(&dir, map).unwrap();
    assert_eq!(back, vec![a2, b]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_candidate_crease_trains_but_does_not_play_until_proved() {
    // The scan carries no need: a site takes the one threshold.
    let crease = Spot {
        shape: superbounce::Shape::Crease,
        need: 0.0,
        ..bounce_spot([2.0, 3.0, 4.0], 5.2, 0.5)
    };
    let site = Site::from_spot(&crease).unwrap();
    assert_eq!(site.need, superbounce::speed::BOUNCE_SPEED);
    assert!(site.candidate && !site.plays());
    // Nothing measured its drop.
    assert!(!site.drop_known());
    // The training's board takes every known site, the candidate too.
    let k = Known::merge(Vec::new(), &[crease]);
    assert_eq!(k.sites.len(), 1);
    // Proved by a game run it plays; a training line proves nothing.
    let proven = Site {
        status: Status::Proven,
        ..site
    };
    assert!(proven.plays());
    let learned = Site {
        line: Some(learned::State::Learned),
        ..site
    };
    assert!(!learned.plays());
    // A slot is no candidate and plays.
    let slot = Site::from_spot(&bounce_spot([0.0; 3], 14.0, 0.8)).unwrap();
    assert!(!slot.candidate && slot.plays());
    assert_eq!(slot.need, 4.3);
}
