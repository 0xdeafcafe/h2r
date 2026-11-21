use std::time::Duration;

use super::*;

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-4, "{a} != {b}");
}

fn spot(kind: Kind, stand: Vec3, proven: bool) -> Spot {
    Spot {
        kind,
        stand,
        edge: [stand, stand],
        status: if proven {
            Status::Proven
        } else {
            Status::Likely
        },
        ..Spot::default()
    }
}

/// Everything past x = 5 is behind a wall.
struct WallAtFive;

impl LineOfSight for WallAtFive {
    fn clear(&self, from: Vec3, to: Vec3) -> bool {
        (from[0] < 5.0) == (to[0] < 5.0)
    }
}

fn input<'a>(now: Instant, down: bool, spots: &'a [Spot]) -> Input<'a> {
    Input {
        now,
        down,
        test_hold: false,
        test_pulse: false,
        test_cancel: false,
        at: Some([0.0, 0.0, 0.0]),
        yaw: Some(0.0),
        spots,
        sensor: Sensor {
            enabled: true,
            range: 10.0,
        },
        nav: None,
    }
}

#[test]
fn a_short_press_is_a_tap_and_a_long_one_a_hold() {
    let mut b = Button::default();
    // Down for TAP_TICKS - 1 ticks, then up: a tap on the release.
    for _ in 0..TAP_TICKS - 1 {
        assert_eq!(b.step(true), None);
    }
    assert!(!b.holding());
    assert_eq!(b.step(false), Some(Press::Tap));
    // Down TAP_TICKS ticks: the hold starts on that tick, once.
    for i in 1..=TAP_TICKS + 5 {
        let press = b.step(true);
        assert_eq!(
            press,
            (i == TAP_TICKS).then_some(Press::HoldStart),
            "tick {i}"
        );
    }
    assert!(b.holding());
    assert_eq!(b.step(false), Some(Press::HoldEnd));
    assert!(!b.holding());
    // Up and up: nothing.
    assert_eq!(b.step(false), None);
}

#[test]
fn the_pulse_front_runs_out_and_each_spot_lights_settles_and_fades() {
    close(pulse_front(0.0), 0.0);
    close(pulse_front(0.5), PULSE_SPEED * 0.5);
    close(pulse_front(-1.0), 0.0);
    let d = 20.0;
    let reach = d / PULSE_SPEED;
    // Not yet reached.
    assert_eq!(pulse_strength(reach - 0.01, d), 0.0);
    // Full as the ring passes, settling.
    close(pulse_strength(reach, d), 1.0);
    let mid = pulse_strength(reach + PULSE_FLASH * 0.5, d);
    assert!(mid < 1.0 && mid > PULSE_SETTLED);
    close(pulse_strength(reach + PULSE_FLASH, d), PULSE_SETTLED);
    close(pulse_strength(reach + PULSE_LIT - 0.01, d), PULSE_SETTLED);
    // Fading, then gone 4 s after it lit.
    close(
        pulse_strength(reach + PULSE_LIT + PULSE_FADE * 0.5, d),
        PULSE_SETTLED * 0.5,
    );
    close(pulse_strength(reach + PULSE_LIT + PULSE_FADE, d), 0.0);
    assert_eq!(
        pulse_strength(reach + PULSE_LIT + PULSE_FADE + 0.01, d),
        0.0
    );
    close(PULSE_LIT + PULSE_FADE, 4.0);
    // The pulse lives until its furthest spot has faded.
    assert!(pulse_alive(reach + 3.9, d));
    assert!(!pulse_alive(reach + 4.0, d));
    assert!(!pulse_alive(-0.1, d));
}

#[test]
fn the_tracker_grows_and_shrinks_on_the_wall_clock() {
    let t0 = Instant::now();
    let g = Grow::rest(t0).toward(1.0, t0);
    close(g.value(t0), 0.0);
    let half = t0 + Duration::from_secs_f32(GROW_SECONDS * 0.5);
    close(g.value(half), 0.5);
    close(g.value(t0 + Duration::from_secs_f32(GROW_SECONDS)), 1.0);
    close(g.value(t0 + Duration::from_secs(5)), 1.0);
    // Let go half way: it shrinks from where it was.
    let back = g.toward(0.0, half);
    close(back.value(half), 0.5);
    close(back.value(half + Duration::from_secs(1)), 0.0);
    // An instant before the start reads as the start.
    close(g.value(t0 - Duration::from_millis(5)), 0.0);
}

#[test]
fn a_tap_fires_a_pulse_and_a_hold_grows_the_map() {
    let spots = [spot(Kind::Bounce, [3.0, 0.0, 0.0], true)];
    let geometry = Geometry::default();
    let mut t = Ticker::new();
    let t0 = Instant::now();
    let tick = |i: u32| t0 + Duration::from_millis(33 * u64::from(i));
    // Three ticks down, then up: a tap at the release.
    for i in 0..3 {
        let f = t.step(&input(tick(i), true, &spots), &WallAtFive, &geometry);
        assert_eq!(f.mode(tick(i)), Mode::None);
    }
    let f = t.step(&input(tick(3), false, &spots), &WallAtFive, &geometry);
    assert_eq!(f.pulse, Some(tick(3)));
    assert_eq!(f.mode(tick(3)), Mode::Pulse);
    assert!(f.map.is_none());
    // Held: after TAP_TICKS ticks the hold starts and the map is made.
    let mut last = None;
    for i in 0..TAP_TICKS {
        last = Some(t.step(&input(tick(10 + i), true, &spots), &WallAtFive, &geometry));
    }
    let last = last.unwrap();
    let now = tick(10 + TAP_TICKS - 1);
    assert!(last.holding);
    assert_eq!(last.mode(now), Mode::Hold);
    assert!(last.map.is_some());
    close(last.grow.to, 1.0);
    // Let go: no tap, the map shrinks and goes.
    let f = t.step(&input(tick(30), false, &spots), &WallAtFive, &geometry);
    assert!(!f.holding);
    close(f.grow.to, 0.0);
    assert!(f.map.is_some(), "kept while it shrinks");
    let later = tick(30) + Duration::from_secs(1);
    let f = t.step(&input(later, false, &spots), &WallAtFive, &geometry);
    assert!(f.map.is_none());
    // The agent's hold and pulse.
    let mut i = input(later, false, &spots);
    i.test_hold = true;
    assert!(t.step(&i, &WallAtFive, &geometry).holding);
    i.test_hold = false;
    i.test_pulse = true;
    let f = t.step(&i, &WallAtFive, &geometry);
    assert_eq!(f.pulse, Some(later));
    i.test_pulse = false;
    i.test_cancel = true;
    assert_eq!(t.step(&i, &WallAtFive, &geometry).pulse, None);
}

#[test]
fn spots_behind_a_wall_are_found_a_few_a_tick_and_drawn_dashed() {
    // Two in sight, two behind the wall at x = 5.
    let spots = [
        spot(Kind::Bounce, [1.0, 0.0, 0.0], true),
        spot(Kind::Crouch, [8.0, 0.0, 0.0], true),
        spot(Kind::Bounce, [-2.0, 1.0, 0.0], false),
        spot(Kind::Bounce, [9.0, 3.0, 0.0], true),
    ];
    let geometry = Geometry::default();
    let mut t = Ticker::new();
    let now = Instant::now();
    // Idle: SIGHT_IDLE a tick.
    let f = t.step(&input(now, false, &spots), &WallAtFive, &geometry);
    assert_eq!(f.tested, SIGHT_IDLE);
    let f = t.step(&input(now, false, &spots), &WallAtFive, &geometry);
    assert_eq!(f.tested, 4);
    let hidden: Vec<Option<bool>> = f.spots.iter().map(|s| s.hidden).collect();
    assert_eq!(hidden, [Some(false), Some(true), Some(false), Some(true)]);
    // Held: every spot marked, the hidden ones dashed and dimmer.
    let mut i = input(now, false, &spots);
    i.test_hold = true;
    let f = t.step(&i, &WallAtFive, &geometry);
    let shown = now + Duration::from_secs(1);
    assert_eq!(f.shown(shown), (4, 2));
    close(f.strength(0, shown), 1.0);
    close(f.strength(1, shown), HIDDEN);
    close(f.strength(2, shown), LIKELY);
    let view = View {
        frame: Arc::new(f),
        colour: [0.5, 0.7, 1.0],
        tracker: TrackerPlace::from_corner(FRAME_BOTTOM_LEFT),
        fit: [1.0, 1.0, 0.0, 0.0],
    };
    let marks = view.world(shown);
    let dashed: Vec<bool> = marks.iter().take(4).map(|m| m.dashed).collect();
    assert_eq!(dashed, [false, true, false, true]);
    // The crouch spot at 8 pairs with the landing at 9, 3: one dashed line.
    assert_eq!(marks.len(), 5);
    assert!(marks[4].dashed && marks[4].label.is_none());
    close(marks[4].alpha, PAIR_ALPHA);
    // A new spot list starts the answers again.
    let f = t.step(&input(now, false, &spots[..2]), &WallAtFive, &geometry);
    assert_eq!(f.tested, 2);
}

#[test]
fn a_spot_on_another_floor_gets_a_chevron_its_way() {
    let eye = [0.0, 0.0, 0.62];
    let centre = [10.0, 0.0, 3.0];
    assert!(level_chevron(Level::Same, centre, eye, 0.2).is_empty());
    let up = level_chevron(Level::Above, centre, eye, 0.2);
    let down = level_chevron(Level::Below, centre, eye, 0.2);
    assert_eq!(up.len(), 2);
    // Above: its tip over the figure and higher than its wings.
    let tip = up[0][1];
    assert!(tip[2] > centre[2] + 0.2 && tip[2] > up[0][0][2]);
    let tip = down[0][1];
    assert!(tip[2] < centre[2] - 0.2 && tip[2] < down[0][0][2]);
    // The chevron and the figure stand across the line of sight (y here).
    assert!(up.iter().flatten().all(|p| p[0] == centre[0]));
    let ring = figure(Kind::Crouch, centre, eye, 0.2);
    assert_eq!(ring.len(), RING_SIDES);
    assert!(ring.iter().flatten().all(|p| p[0] == centre[0]));
    assert_eq!(figure(Kind::Bounce, centre, eye, 0.2).len(), 4);

    // Through the ticker: a spot 2 units up is above, one 2 down below.
    let spots = [
        spot(Kind::Bounce, [3.0, 0.0, 2.0], true),
        spot(Kind::Bounce, [3.0, 1.0, -2.0], true),
        spot(Kind::Bounce, [3.0, 2.0, 0.5], true),
    ];
    let f = Ticker::new().step(
        &input(Instant::now(), false, &spots),
        &WallAtFive,
        &Geometry::default(),
    );
    let levels: Vec<Level> = f.spots.iter().map(|s| s.level).collect();
    assert_eq!(levels, [Level::Above, Level::Below, Level::Same]);
}

#[test]
fn marks_shrink_with_distance_and_far_ones_lose_their_number() {
    close(mark_size(0.0), MARK_MIN);
    close(mark_size(10.0), 10.0 * MARK_ANGLE);
    close(mark_size(1000.0), MARK_MAX);
    assert_eq!(metres(1.0), "3 m");
    assert_eq!(metres(0.01), "1 m");
    assert_eq!(metres(MAP_RANGE), "50 m");
    let near = LABEL_METRES / METRES_PER_UNIT - 0.5;
    let spots = [
        spot(Kind::Bounce, [near, 0.0, 0.0], true),
        spot(Kind::Bounce, [near + 2.0, 0.0, 0.0], true),
    ];
    let mut i = input(Instant::now(), false, &spots);
    i.test_hold = true;
    let f = Ticker::new().step(&i, &WallAtFive, &Geometry::default());
    let view = View {
        frame: Arc::new(f),
        colour: [1.0; 3],
        tracker: TrackerPlace::from_corner(FRAME_BOTTOM_LEFT),
        fit: [1.0, 1.0, 0.0, 0.0],
    };
    let marks = view.world(i.now + Duration::from_secs(1));
    assert!(marks[0].label.is_some());
    assert!(marks[1].label.is_none());
}

#[test]
fn blips_turn_with_the_ticks_yaw_and_far_spots_sit_on_the_rim() {
    let vs = |stand: Vec3, proven: bool| ViewSpot {
        stand,
        kind: Kind::Bounce,
        proven,
        level: Level::Same,
        distance: stand[0].hypot(stand[1]),
        hidden: None,
        pair: None,
        nav: false,
    };
    // Facing +X: a spot 5 ahead is straight up the tracker at half range.
    let spots = [vs([5.0, 0.0, 0.0], true)];
    let b = blips([0.0; 3], 0.0, 10.0, &spots, Rim::Nearest(RIM_MAX));
    close(b[0].at[0], 0.0);
    close(b[0].at[1], 0.5);
    assert!(!b[0].rim);
    // Facing +Y (turned left a quarter): it is now to the right.
    let b = blips(
        [0.0; 3],
        std::f32::consts::FRAC_PI_2,
        10.0,
        &spots,
        Rim::Nearest(RIM_MAX),
    );
    close(b[0].at[0], 0.5);
    close(b[0].at[1], 0.0);
    // +Y from facing +X is to the left.
    let p = place([0.0; 3], 0.0, [0.0, 4.0, 0.0]);
    close(p[0], -4.0);
    close(p[1], 0.0);

    // Out of range: on the rim, a unit vector, the nearest RIM_MAX only,
    // after the ones in range.
    let mut many = vec![vs([3.0, 0.0, 0.0], true)];
    for i in 0..10 {
        many.push(vs([0.0, -(20.0 + i as f32), 0.0], i % 2 == 0));
    }
    let b = blips([0.0; 3], 0.0, 10.0, &many, Rim::Nearest(RIM_MAX));
    assert_eq!(b.len(), 1 + RIM_MAX);
    assert!(!b[0].rim && b[1..].iter().all(|b| b.rim));
    let rim = b[1];
    assert_eq!(rim.spot, 1, "the nearest out of range first");
    close(rim.at[0], 1.0);
    close(rim.at[1], 0.0);
    // The map's rim: every proven one.
    let b = blips([0.0; 3], 0.0, 10.0, &many, Rim::Proven);
    assert_eq!(b.iter().filter(|b| b.rim).count(), 5);
    assert!(b.iter().filter(|b| b.rim).all(|b| b.proven));
    // No range, no blips.
    assert!(blips([0.0; 3], 0.0, 0.0, &many, Rim::Proven).is_empty());
}

#[test]
fn the_tracker_is_the_retail_one_and_the_map_grows_from_its_corner() {
    let t = TrackerPlace::from_corner(FRAME_BOTTOM_LEFT);
    // rects::TRACKER: x 47..130, y 360..443.
    close(t.centre[0], 88.5);
    close(t.centre[1], 401.5);
    close(t.centre[0] - t.radius, 47.0);
    close(t.centre[1] + t.radius, 443.0);
    let (c, r) = t.map(0.0);
    assert_eq!((c, r), (t.centre, t.radius));
    let (c, r) = t.map(1.0);
    close(r, t.radius * GROW);
    // The bottom-left corner stays put.
    close(c[0] - r, 47.0);
    close(c[1] + r, 443.0);
}

#[test]
fn the_map_is_made_from_the_collision_near_the_player() {
    use superbounce::Poly;
    let quad = |x0: f32, y0: f32, x1: f32, y1: f32, z: f32| Poly {
        verts: vec![[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]],
        normal: [0.0, 0.0, 1.0],
    };
    // The ground under the player, a ledge ahead, a far floor out of the
    // map's reach and a roof far above.
    let polys = [
        quad(-4.0, -4.0, 4.0, 4.0, 0.0),
        quad(6.0, -1.0, 8.0, 1.0, 1.0),
        quad(100.0, 0.0, 102.0, 2.0, 0.0),
        quad(-4.0, -4.0, 4.0, 4.0, 20.0),
    ];
    let geometry = Geometry::new(&polys);
    let spots = [
        spot(Kind::Crouch, [7.0, 0.0, 1.0], true),
        spot(Kind::Bounce, [2.0, 0.0, 0.0], true),
        spot(Kind::Bounce, [80.0, 0.0, 0.0], true),
    ];
    let mut i = input(Instant::now(), false, &spots);
    i.test_hold = true;
    let f = Ticker::new().step(&i, &geometry, &geometry);
    let map = f.map.as_ref().unwrap();
    assert_eq!(map.floors.len(), 2);
    // Lowest first; the ledge ahead is up the map (facing +X).
    close(map.floors[1].1, 1.0);
    assert!(
        map.floors[1]
            .0
            .iter()
            .all(|p| p[1] > 0.0 && p[0].abs() < 0.1)
    );
    // Everything inside the unit circle.
    assert!(
        map.floors
            .iter()
            .flat_map(|f| &f.0)
            .chain(map.edges.iter().flatten())
            .all(|p| p[0].hypot(p[1]) <= 1.0 + 1e-4)
    );
    // The ground's outline (4) and the ledge's (4).
    assert_eq!(map.edges.len(), 8);
    // The crouch spot on the ledge pairs with the landing at 2, 0.
    assert_eq!(map.pairs.len(), 1);
    // Two in range, the far proven one on the rim.
    assert_eq!(map.blips.len(), 3);
    assert!(map.blips[2].rim);
}

#[test]
fn the_same_instant_draws_the_same_picture_however_many_presents() {
    let spots = [
        spot(Kind::Bounce, [3.0, 1.0, 0.0], true),
        spot(Kind::Crouch, [6.0, -2.0, 1.5], false),
        spot(Kind::Bounce, [30.0, 0.0, 0.0], true),
    ];
    let t0 = Instant::now();
    let mut i = input(t0, false, &spots);
    i.test_pulse = true;
    let f = Ticker::new().step(&i, &WallAtFive, &Geometry::default());
    let view = View {
        frame: Arc::new(f),
        colour: [0.4, 0.6, 1.0],
        tracker: TrackerPlace::from_corner(FRAME_BOTTOM_LEFT),
        fit: [0.75, 1.0, 0.0, 0.0],
    };
    let at = t0 + Duration::from_millis(200);
    let once = (view.world(at), view.hud(at));
    // A 120 Hz window presents the same image many times in between.
    for k in 0..20 {
        let _ = view.world(t0 + Duration::from_millis(8 * k));
        let _ = view.hud(t0 + Duration::from_millis(8 * k));
    }
    assert_eq!((view.world(at), view.hud(at)), once);
    // And the pulse moves on with the clock, not the presents: at 0.2 s
    // the ring is 8 units out, past the spots at 3 and 6.5 but not 30.
    let lit: Vec<f32> = (0..3).map(|i| view.frame.strength(i, at)).collect();
    assert!(lit[0] > 0.0 && lit[1] > 0.0 && lit[2] == 0.0);
    // The tracker's ring is drawn while it is inside the range.
    assert!(
        once.1
            .iter()
            .any(|s| matches!(s, Shape::Path { closed: true, points, .. } if points.len() == 48))
    );
}

#[test]
fn the_hud_fit_moves_points_and_scales_widths_with_the_height() {
    let s = Shape::Path {
        points: vec![[88.5, 401.5]],
        closed: false,
        width: 2.0,
        dashed: false,
        color: [1.0; 3],
        alpha: 1.0,
    };
    let Shape::Path { points, width, .. } = s.fitted([0.75, 1.0, 0.0, 0.0]) else {
        unreachable!()
    };
    close(points[0][0], 88.5 * 0.75);
    close(points[0][1], 401.5);
    close(width, 2.0);
    let t = Shape::Text {
        at: [10.0, 20.0],
        text: "50 m".into(),
        color: [1.0; 3],
        alpha: 1.0,
        pixel: 1.0,
    };
    let Shape::Text { at, pixel, .. } = t.fitted([1.0, 0.5, 4.0, 2.0]) else {
        unreachable!()
    };
    assert_eq!(at, [14.0, 12.0]);
    close(pixel, 0.5);
}

#[test]
fn blips_fade_as_the_map_grows_and_the_map_has_a_rim_and_a_range() {
    let spots = [spot(Kind::Bounce, [3.0, 0.0, 0.0], true)];
    let mut t = Ticker::new();
    let t0 = Instant::now();
    let f = t.step(&input(t0, false, &spots), &WallAtFive, &Geometry::default());
    let view = |f: Frame| View {
        frame: Arc::new(f),
        colour: [1.0; 3],
        tracker: TrackerPlace::from_corner(FRAME_BOTTOM_LEFT),
        fit: [1.0, 1.0, 0.0, 0.0],
    };
    // Idle: just the blip, a filled diamond at 3/10 of the way up.
    let shapes = view(f).hud(t0);
    assert_eq!(shapes.len(), 1);
    let Shape::Fill { points, .. } = &shapes[0] else {
        panic!("{shapes:?}")
    };
    close(points[0][1], 401.5 - 0.3 * BLIP_RADIUS - BLIP_SIZE);
    // With the tracker off in the variant, nothing.
    let mut off = input(t0, false, &spots);
    off.sensor.enabled = false;
    let f = t.step(&off, &WallAtFive, &Geometry::default());
    assert!(view(f.clone()).hud(t0).is_empty());
    assert!(!f.drawable(t0));
    // Held and grown: no tracker blip, the map's rim and its range.
    let mut held = input(t0, false, &spots);
    held.test_hold = true;
    let f = t.step(&held, &WallAtFive, &Geometry::default());
    let grown = view(f).hud(t0 + Duration::from_secs(1));
    assert!(
        grown
            .iter()
            .any(|s| matches!(s, Shape::Text { text, .. } if text == "50 m"))
    );
    assert!(
        grown
            .iter()
            .any(|s| matches!(s, Shape::Path { closed: true, points, .. } if points.len() == 64))
    );
}

#[test]
fn the_reply_counts_what_shows() {
    let spots = [
        spot(Kind::Bounce, [3.0, 0.0, 0.0], true),
        spot(Kind::Crouch, [8.0, 0.0, 0.0], false),
    ];
    let mut t = Ticker::new();
    let now = Instant::now();
    t.step(
        &input(now, false, &spots),
        &WallAtFive,
        &Geometry::default(),
    );
    let mut i = input(now, false, &spots);
    i.test_hold = true;
    let f = t.step(&i, &WallAtFive, &Geometry::default());
    let r = reply(Some(&f), true, now + Duration::from_secs(1));
    assert_eq!(r.mode, "hold");
    assert_eq!((r.spots, r.marks, r.hidden, r.tested), (2, 2, 1, 2));
    assert!(r.test_hold);
    close(r.grow as f32, 1.0);
    assert_eq!(r.tracker.blips.len(), 2);
    assert_eq!(r.tracker.blips[0].kind, "bounce");
    assert_eq!(r.tracker.blips[0].level, "same");
    assert!(r.map.is_some());
    // Off: nothing.
    let r = reply(Some(&f), false, now);
    assert!(!r.active && r.mode == "none" && r.spots == 0 && r.map.is_none());
}
