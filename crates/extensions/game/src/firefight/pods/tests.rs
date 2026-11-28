//! The pods' machine over invented kits and players: no game.
use super::*;

fn fall(x: f32) -> PodFall {
    PodFall { top: [x, 0.0, 40.0], ground: [x, 0.0, 0.0] }
}

/// A kit with `pods` player pods and a fall at starts 0, 1, ... by x = 10 * n.
fn kit(pods: usize, falls: usize) -> KitLayout {
    KitLayout {
        squads: Default::default(),
        zones: Vec::new(),
        bases: Vec::new(),
        characters: 1,
        classes: Vec::new(),
        factions: Default::default(),
        team: 3,
        hunt_orders: Vec::new(),
        hold_orders: Vec::new(),
        ships: [None, None],
        paths: Vec::new(),
        pods: Vec::new(),
        flood_pods: Vec::new(),
        player_pods: (0..pods as i16).collect(),
        falls: Vec::new(),
        player_falls: (0..falls).map(|n| (n as u16, fall(10.0 * n as f32))).collect(),
    }
}

fn player(slot: u8, at: [f32; 3]) -> Seen {
    Seen { slot, alive: true, position: Some(at), unit: Some(0x100 + u32::from(slot)) }
}

fn pool(n: usize) -> Vec<PodSeen> {
    (0..n)
        .map(|i| PodSeen { object: Some(0x200 + i as u32), position: Some([-50.0, 0.0, 0.0]) })
        .collect()
}

fn drops(acts: &[Act]) -> Vec<(usize, u32)> {
    acts.iter()
        .filter_map(|a| match a {
            Act::Drop { pod, unit, .. } => Some((*pod, *unit)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_respawn_under_open_sky_falls_by_pod_and_ejects_on_landing() {
    let mut d = PodDrops::new(&kit(2, 2), true);
    let me = [10.5, 0.0, 0.0];
    let mut pods = pool(2);
    // The player spawns at start 1; the pod is taken for them.
    let acts = d.step(5, &[player(0, me)], &pods);
    assert_eq!(drops(&acts), [(0, 0x100)]);
    assert_eq!(acts.len(), 1);
    let Act::Drop { fall: f, object, .. } = acts[0] else { unreachable!() };
    assert_eq!((f, object), (fall(10.0), 0x200));
    // The call succeeded, the flight began.
    d.dropped(0, 6);
    assert_eq!(d.counts().spawns, 1);
    // Falling: the pod is in the air, nothing to do.
    pods[0].position = Some([10.0, 0.0, 20.0]);
    assert!(d.step(40, &[player(0, [10.0, 0.0, 20.0])], &pods).is_empty());
    // It lands at tick 81; the rider is ejected 15 ticks later.
    pods[0].position = Some([10.0, 0.0, 0.0]);
    assert!(d.step(81, &[player(0, [10.0, 0.0, 0.0])], &pods).is_empty());
    let acts = d.step(81 + LAND_TICKS, &[player(0, [10.0, 0.0, 0.0])], &pods);
    assert_eq!(acts, [Act::Eject { unit: 0x100 }]);
    // The wreck stays, then the pod is parked and free again.
    assert!(d.step(100, &[player(0, [10.0, 0.0, 0.0])], &pods).is_empty());
    let acts = d.step(96 + WRECK_TICKS, &[player(0, [10.0, 0.0, 0.0])], &pods);
    assert_eq!(acts, [Act::Park { object: 0x200, at: [0.0, 0.0, 40.0] }]);
    let acts = d.step(500, &[player(0, [10.0, 0.0, 0.0])], &pods);
    assert!(acts.is_empty());
    assert!(d.counts().fallbacks.is_empty());
}

#[test]
fn a_start_without_a_fall_spawns_normally() {
    let mut d = PodDrops::new(&kit(2, 2), true);
    // 5 units from the nearest fall's ground: the engine's spawn stands.
    let acts = d.step(1, &[player(0, [4.5, 0.0, 0.0])], &pool(2));
    assert!(acts.is_empty());
    assert_eq!(d.counts().fallbacks[&Reason::NoFall], 1);
    assert_eq!(d.counts().spawns, 0);
    // The same player later alive at the same place is not a new spawn.
    assert!(d.step(2, &[player(0, [4.5, 0.0, 0.0])], &pool(2)).is_empty());
    assert_eq!(d.counts().fallbacks[&Reason::NoFall], 1);
}

#[test]
fn a_blocked_column_spawns_normally() {
    let mut d = PodDrops::new(&kit(2, 1), true);
    let acts = d.step(1, &[player(0, [0.5, 0.0, 0.0])], &pool(2));
    assert_eq!(drops(&acts), [(0, 0x100)]);
    // The drop call found the column blocked: the player stands, the pod
    // is free for the next spawn, nobody is ejected.
    d.failed(0, Reason::Blocked);
    assert_eq!(d.counts().fallbacks[&Reason::Blocked], 1);
    assert_eq!(d.counts().spawns, 0);
    let acts = d.step(2, &[player(0, [0.5, 0.0, 0.0]), player(1, [0.2, 0.0, 0.0])], &pool(2));
    assert_eq!(drops(&acts), [(0, 0x101)], "the same pod again, for the next spawn");
    assert!(!acts.iter().any(|a| matches!(a, Act::Eject { .. })));
}

#[test]
fn more_players_than_pods_spawn_the_rest_normally() {
    let mut d = PodDrops::new(&kit(8, 1), true);
    let players: Vec<Seen> = (0..9).map(|s| player(s, [0.0, 0.0, 0.0])).collect();
    let pods = pool(8);
    let mut all = Vec::new();
    // The whole team spawns at once: the first drops now, the rest 10 ticks
    // apart, and the ninth finds no pod.
    all.extend(drops(&d.step(10, &players, &pods)));
    assert_eq!(all.len(), 1);
    assert_eq!(d.counts().fallbacks[&Reason::NoFreePod], 1);
    for tick in 11..=10 + 7 * DROP_GAP {
        let acts = d.step(tick, &players, &pods);
        if !acts.is_empty() {
            assert_eq!((tick - 10) % DROP_GAP, 0, "drops are {DROP_GAP} ticks apart");
        }
        all.extend(drops(&acts));
    }
    assert_eq!(all.len(), 8);
    let mut pods_used: Vec<usize> = all.iter().map(|(p, _)| *p).collect();
    pods_used.sort();
    assert_eq!(pods_used, (0..8).collect::<Vec<_>>());
    // The ninth player never had a drop.
    assert!(!all.iter().any(|(_, unit)| *unit == 0x108));
}

#[test]
fn a_pod_that_never_arrives_ejects_its_rider() {
    let mut d = PodDrops::new(&kit(1, 1), true);
    let pods = pool(1);
    let me = [player(0, [0.0, 0.0, 0.0])];
    assert_eq!(drops(&d.step(1, &me, &pods)).len(), 1);
    d.dropped(0, 2);
    // The pod's position never reaches the ground.
    for tick in 3..=2 + FALL_TICKS + WATCHDOG_TICKS {
        assert!(d.step(tick, &me, &pods).is_empty(), "tick {tick}");
    }
    let acts = d.step(3 + FALL_TICKS + WATCHDOG_TICKS, &me, &pods);
    assert_eq!(
        acts,
        [Act::Eject { unit: 0x100 }, Act::Park { object: 0x200, at: [0.0, 0.0, 40.0] }]
    );
    assert_eq!(d.counts().fallbacks[&Reason::NeverLanded], 1);
}

#[test]
fn pods_off_never_touch_the_spawn_pick() {
    let kit = kit(2, 2);
    let teams = [0, 0];
    // On, the team's open starts are the pick; off, none: the engine's.
    assert_eq!(spawn_pick(true, &kit, &teams, 0), [0, 1].into());
    assert!(spawn_pick(false, &kit, &teams, 0).is_empty());
    // Another team's starts are not this team's.
    assert!(spawn_pick(true, &kit, &teams, 1).is_empty());
    let mut off = PodDrops::new(&kit, false);
    assert!(!off.enabled());
    assert!(off.open_starts().is_empty());
    assert!(off.step(1, &[player(0, [0.0, 0.0, 0.0])], &pool(2)).is_empty());
    assert_eq!(off.counts(), &Counts::default());
}

#[test]
fn the_row_is_hidden_without_open_starts() {
    assert_eq!(row(Some(&kit(2, 1))), Row::Shown);
    assert!(matches!(row(Some(&kit(2, 0))), Row::Hidden(why) if why.contains("open sky")));
    assert!(matches!(row(Some(&kit(0, 1))), Row::Hidden(why) if why.contains("pod")));
    assert!(matches!(row(None), Row::Hidden(_)));
    // A hidden row is off whatever it is set to.
    assert!(!PodDrops::new(&kit(2, 0), true).enabled());
    // Nothing is preferred on a map with no open start.
    assert!(spawn_pick(true, &kit(2, 0), &[0], 0).is_empty());
}

/// An invented unit, graph and tag table for the seat test: the graph has
/// the modes `labels`.
fn seat_guest(labels: &[u32], any_seat: u8) -> (guest_layout::testing::FakeGuest, u32) {
    let mut g = guest_layout::testing::FakeGuest::new();
    let (unit, state_offset, graph_tag, modes, table) = (0x10_0000, 0x300u16, 0x20_0000, 0x21_0000, 0x30_0000);
    g.put_u8(unit + ANY_SEAT, any_seat)
        .put_u16(unit + ANIMATION_STATE, state_offset)
        .put_u32(unit + u32::from(state_offset) + STATE_GRAPH, 0xe000_0005)
        .put_u32(TAG_TABLE, table)
        .put_u32(table + 5 * 16 + 8, graph_tag)
        .put_u32(graph_tag + GRAPH_MODES, labels.len() as u32)
        .put_u32(graph_tag + GRAPH_MODES + 4, modes);
    for (i, label) in labels.iter().enumerate() {
        g.put_u32(modes + i as u32 * MODE_STRIDE, *label);
    }
    (g, unit)
}

/// The rider takes a seat only when its graph has the seat's mode, unless
/// it takes any seat: the test the engine makes before it seats anyone.
#[test]
fn a_rider_takes_a_seat_its_graph_has_a_mode_for() {
    let (g, unit) = seat_guest(&[0x0a00_0001, 0x0a00_0002], 0);
    assert_eq!(takes_seat(&g, unit, 0x0a00_0002), Some(true));
    assert_eq!(takes_seat(&g, unit, 0x0a00_0003), Some(false), "no such mode");
    let (g, unit) = seat_guest(&[], 1);
    assert_eq!(takes_seat(&g, unit, 0x0a00_0003), Some(true), "takes any seat");
}
