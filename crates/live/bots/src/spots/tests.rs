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
    assert_eq!(k.crouches, vec![[5.0, 5.0, 0.0]]);
}

#[test]
fn the_nearest_crouch_spot_is_by_straight_line() {
    let k = Known {
        sites: Vec::new(),
        crouches: vec![[10.0, 0.0, 0.0], [2.0, 2.0, 0.0]],
    };
    assert_eq!(k.nearest_crouch([0.0, 0.0, 0.0]), Some([2.0, 2.0, 0.0]));
    assert_eq!(Known::default().nearest_crouch([0.0; 3]), None);
}

#[test]
fn the_spots_file_round_trips_and_refuses_damage() {
    let spots = vec![bounce_spot([1.0, 2.0, 3.0], 12.5, 0.7), crouch_spot([4.0, 5.0, 6.0])];
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
