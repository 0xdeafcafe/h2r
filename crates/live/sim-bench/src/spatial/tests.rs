use super::*;
use crate::metrics::Tally;

/// Polygon = the integer part of X (none below 0); a wall at X = 10.
struct Strip;

impl Ground for Strip {
    fn poly(&self, p: V3) -> Option<u32> {
        (p[0] >= 0.0).then(|| p[0] as u32)
    }
    fn sees(&self) -> bool {
        true
    }
    fn clear(&self, a: V3, b: V3) -> bool {
        (a[0] < 10.0) == (b[0] < 10.0)
    }
}

fn place(x: f32, poly: Option<u32>) -> Place {
    Place {
        pos: [x, 1.0, 2.0],
        poly,
    }
}

fn party(player: u8, at: Option<Place>) -> Party {
    Party {
        player,
        team: Some(1),
        at,
        weapon: Some(Weapon {
            datum: 0xe00a_0001,
            definition: None,
        }),
    }
}

fn every_kind() -> Trace {
    Trace {
        header: Header {
            map: "lockout".into(),
            game_type: "slayer".into(),
            seed: 7,
            team_play: true,
            start_tick: 100,
            end_tick: 9100,
            starts: vec![Start {
                place: place(1.0, Some(1)),
                team: Some(0),
            }],
            items: vec![
                Item {
                    place: place(4.0, None),
                    respawn_s: Some(30),
                },
                Item {
                    place: place(5.0, Some(5)),
                    respawn_s: None,
                },
            ],
            mesh: Some(Mesh {
                centers: vec![[0.5, 0.0, 0.0], [1.5, 0.0, 0.0]],
                links: vec![(0, 1), (1, 0)],
            }),
        },
        events: vec![
            Event::Sample {
                tick: 100,
                player: 0,
                at: place(1.0, Some(1)),
                yaw: 1.5,
                pitch: -0.25,
            },
            Event::Spawn {
                tick: 101,
                player: 3,
                team: None,
                at: place(2.0, None),
                start: Some(0),
                nearest: Some(Nearest {
                    player: 0,
                    distance: 12.5,
                }),
                in_sight: Some(2),
            },
            Event::Spawn {
                tick: 101,
                player: 4,
                team: Some(-1),
                at: place(2.0, Some(2)),
                start: None,
                nearest: None,
                in_sight: None,
            },
            Event::Damage {
                tick: 102,
                attacker: Some(party(0, Some(place(1.0, Some(1))))),
                victim: party(3, None),
                amount: 0.5,
            },
            Event::Damage {
                tick: 102,
                attacker: None,
                victim: party(3, Some(place(2.0, None))),
                amount: 0.0,
            },
            Event::Kill {
                tick: 103,
                killer: Some(party(0, Some(place(1.0, Some(1))))),
                victim: party(3, Some(place(2.0, None))),
                kind: KillKind::Kill,
            },
            Event::Kill {
                tick: 104,
                killer: None,
                victim: party(4, None),
                kind: KillKind::World,
            },
            Event::Pickup {
                tick: 105,
                player: 0,
                at: place(4.0, Some(4)),
                weapon: Some(Weapon {
                    datum: 9,
                    definition: Some(0xe123_0004),
                }),
                item: Some(0),
            },
            Event::Pickup {
                tick: 106,
                player: 0,
                at: place(4.0, Some(4)),
                weapon: None,
                item: None,
            },
            Event::Stuck {
                tick: 107,
                player: 1,
                at: place(6.0, Some(6)),
                toward: Some([9.0, 9.0, 0.0]),
                ticks: 40,
            },
            Event::Repath {
                tick: 108,
                player: 1,
                at: place(6.0, Some(6)),
                toward: None,
            },
        ],
    }
}

#[test]
fn the_file_round_trips() {
    let t = every_kind();
    let bytes = t.to_bytes();
    assert_eq!(&bytes[..4], MAGIC);
    assert_eq!(Trace::from_bytes(&bytes).unwrap(), t);
    // With no mesh and no events too.
    let bare = Trace {
        header: Header {
            map: "ascension".into(),
            ..Default::default()
        },
        events: vec![],
    };
    assert_eq!(Trace::from_bytes(&bare.to_bytes()).unwrap(), bare);
    // Through a file.
    let dir = std::env::temp_dir().join(format!("h2-spatial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(FILE_NAME);
    t.write(&path).unwrap();
    assert_eq!(Trace::read(&path).unwrap(), t);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_damaged_file_is_refused() {
    let bytes = every_kind().to_bytes();
    assert!(Trace::from_bytes(&bytes[..bytes.len() - 1]).unwrap_err().contains("ends early"));
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(Trace::from_bytes(&longer).unwrap_err().contains("after the last event"));
    let mut magic = bytes.clone();
    magic[0] = b'X';
    assert!(Trace::from_bytes(&magic).unwrap_err().contains("H2SP"));
    let mut version = bytes.clone();
    version[4] = 9;
    assert!(Trace::from_bytes(&version).unwrap_err().contains("version 9"));
    // A count bigger than the file is not allocated.
    let mut empty = Trace::default().to_bytes();
    let n = empty.len();
    empty[n - 4..].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Trace::from_bytes(&empty).unwrap_err().contains("more than the file holds"));
}

#[test]
fn the_trace_sits_beside_the_report() {
    let p = |s: &str| trace_path(Path::new(s));
    assert_eq!(p("/r/runs/a-s1/report.json"), Path::new("/r/runs/a-s1/spatial.bin"));
    assert_eq!(p("/tmp/h2sim/duel.report.json"), Path::new("/tmp/h2sim/duel.spatial.bin"));
    assert_eq!(p("/tmp/out.json"), Path::new("/tmp/out.spatial.bin"));
}

fn pose(player: u8, team: i32, alive: bool, x: f32) -> Pose {
    Pose {
        player,
        team: Some(team),
        alive,
        pos: [x, 1.0, 2.0],
        facing: [0.0, 1.0, 0.0],
        weapon: Some(Weapon {
            datum: 0x100 + u32::from(player),
            definition: Some(0xd00 + u32::from(player)),
        }),
        weapons: Some([0x100 + u32::from(player), u32::MAX, u32::MAX, u32::MAX]),
        grenades: Some(2),
    }
}

fn recorder() -> Recorder {
    let mut r = Recorder::new(Header {
        map: "lockout".into(),
        game_type: "slayer".into(),
        team_play: true,
        ..Default::default()
    });
    r.set_places(
        vec![
            Start {
                place: Place {
                    pos: [1.0, 1.0, 2.0],
                    poly: None,
                },
                team: Some(0),
            },
            Start {
                place: Place {
                    pos: [12.0, 1.0, 2.0],
                    poly: None,
                },
                team: Some(1),
            },
        ],
        vec![Item {
            place: Place {
                pos: [5.0, 1.5, 2.0],
                poly: None,
            },
            respawn_s: None,
        }],
    );
    r
}

fn count(r: &Recorder, f: fn(&Event) -> bool) -> usize {
    r.events().iter().filter(|e| f(e)).count()
}

#[test]
fn each_event_is_written_once_with_its_places() {
    let g = Strip;
    let mut r = recorder();
    r.start(600);
    // Tick 600: both spawn, one on each side of the wall.
    r.tick(600, &[pose(0, 0, true, 1.0), pose(1, 1, true, 12.0)], &g);
    let spawns: Vec<_> = r
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Spawn { .. }))
        .cloned()
        .collect();
    assert_eq!(spawns.len(), 2);
    assert_eq!(
        spawns[0],
        Event::Spawn {
            tick: 600,
            player: 0,
            team: Some(0),
            at: Place {
                pos: [1.0, 1.0, 2.0],
                poly: Some(1)
            },
            start: Some(0),
            nearest: Some(Nearest {
                player: 1,
                distance: 11.0
            }),
            in_sight: Some(0),
        }
    );
    // Ticks 601..=612: player 0 walks to the item; samples at 600, 606, 612.
    for t in 601..=612u32 {
        let mut a = pose(0, 0, true, 1.0 + (t - 600) as f32 / 3.0);
        if t >= 611 {
            // A rocket launcher from the placement at X 5, now held.
            a.weapons = Some([0x100, 0x777, u32::MAX, u32::MAX]);
            a.weapon = Some(Weapon {
                datum: 0x777,
                definition: Some(0xdead),
            });
        }
        if t >= 612 {
            a.grenades = Some(4);
        }
        r.tick(t, &[a, pose(1, 1, true, 12.0)], &g);
        // Player 1 is stuck from 605 to 608, and repaths at 607.
        r.brain(1, (605..=608).contains(&t), u32::from(t >= 607), Some([20.0, 0.0, 0.0]), &g);
    }
    assert_eq!(count(&r, |e| matches!(e, Event::Spawn { .. })), 2);
    assert_eq!(count(&r, |e| matches!(e, Event::Sample { player: 0, .. })), 3);
    assert_eq!(count(&r, |e| matches!(e, Event::Sample { player: 1, .. })), 3);
    let pickups: Vec<_> = r
        .events()
        .iter()
        .filter_map(|e| match e {
            Event::Pickup {
                tick, weapon, item, at, ..
            } => Some((*tick, *weapon, *item, at.poly)),
            _ => None,
        })
        .collect();
    assert_eq!(
        pickups,
        [
            (
                611,
                Some(Weapon {
                    datum: 0x777,
                    definition: Some(0xdead)
                }),
                Some(0),
                Some(4)
            ),
            // Grenades from the same placement.
            (612, None, Some(0), Some(5)),
        ]
    );
    assert_eq!(count(&r, |e| matches!(e, Event::Repath { .. })), 1);
    // The spell ended at 609: written once, four ticks long, where it began.
    let stuck: Vec<_> = r
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Stuck { .. }))
        .collect();
    assert_eq!(
        stuck,
        [&Event::Stuck {
            tick: 605,
            player: 1,
            at: Place {
                pos: [12.0, 1.0, 2.0],
                poly: Some(12)
            },
            toward: Some([20.0, 0.0, 0.0]),
            ticks: 4,
        }]
    );
    // Inside tick 613: a hit and the kill, with both places and the
    // weapon held at the last tick.
    r.damage(613, Some(0), 1, 0.75, &g);
    r.kill(613, Some(0), 1, false, &g);
    let Some(Event::Kill {
        killer: Some(k),
        victim: v,
        kind: KillKind::Kill,
        ..
    }) = r.events().last().cloned()
    else {
        panic!("no kill: {:?}", r.events().last());
    };
    assert_eq!(k.at.unwrap().poly, Some(5));
    assert_eq!(k.weapon.unwrap().definition, Some(0xdead));
    assert_eq!(v.at.unwrap().pos, [12.0, 1.0, 2.0]);
    let (a, b) = r.events().last().unwrap().segment().unwrap();
    assert!((a.distance(&b) - 7.0).abs() < 1e-4);
    // Tick 613: player 1 is dead (no new event), then respawns at 618.
    r.tick(613, &[pose(0, 0, true, 5.0), pose(1, 1, false, 12.0)], &g);
    r.tick(618, &[pose(0, 0, true, 5.0), pose(1, 1, true, 1.2)], &g);
    let last_spawn = r
        .events()
        .iter()
        .rev()
        .find(|e| matches!(e, Event::Spawn { .. }))
        .unwrap();
    let Event::Spawn {
        player: 1,
        start,
        in_sight,
        ..
    } = last_spawn
    else {
        panic!("{last_spawn:?}");
    };
    // Spawned on the other team's start, in sight of player 0.
    assert_eq!((*start, *in_sight), (Some(0), Some(1)));
    assert_eq!(count(&r, |e| matches!(e, Event::Spawn { .. })), 3);
    assert_eq!(count(&r, |e| matches!(e, Event::Kill { .. })), 1);
    assert_eq!(count(&r, |e| matches!(e, Event::Damage { .. })), 1);
    // An open spell is closed by the finish; the places get polygons.
    r.brain(0, true, 0, None, &g);
    let t = r.finish(&g, None);
    assert_eq!(t.count(|e| matches!(e, Event::Stuck { player: 0, .. })), 1);
    assert_eq!(t.header.starts[1].place.poly, Some(12));
    assert_eq!(t.header.items[0].place.poly, Some(5));
    assert_eq!((t.header.start_tick, t.header.end_tick), (600, 618));
}

#[test]
fn the_trace_kills_are_the_tallys() {
    let g = NoGround;
    let mut r = recorder();
    let mut tally = Tally::new();
    r.start(10);
    tally.start(10);
    let kills = [
        (11, Some(0), 1, false),
        (12, Some(1), 1, false),
        (13, None, 0, false),
        (14, Some(2), 3, true),
    ];
    for (tick, killer, victim, betrayal) in kills {
        tally.kill(tick, killer, victim, betrayal);
        r.kill(tick, killer, victim, betrayal, &g);
    }
    let t = r.finish(&g, None);
    t.agrees_with(tally.kills()).unwrap();
    assert!(t.agrees_with(&tally.kills()[1..]).is_err());
    // No ground: no polygons, no sight count.
    assert!(t.events.iter().all(|e| match e {
        Event::Kill { victim, .. } => victim.at.is_none(),
        _ => false,
    }));
}

#[test]
fn a_restart_drops_what_came_before() {
    let g = NoGround;
    let mut r = recorder();
    r.start(0);
    r.tick(0, &[pose(0, 0, true, 1.0)], &g);
    r.kill(1, None, 0, false, &g);
    r.start(50);
    assert!(r.events().is_empty());
    r.tick(50, &[pose(0, 0, true, 1.0)], &g);
    // Alive again after the restart is a spawn again.
    assert_eq!(
        r.events()
            .iter()
            .filter(|e| matches!(e, Event::Spawn { .. }))
            .count(),
        1
    );
}
