use super::*;

fn flag(kind: u16, team: u16, x: f32) -> FlagRow {
    FlagRow {
        position: [x, 0.0, 1.0],
        kind,
        team,
    }
}

fn start(team: i16, x: f32, y: f32) -> StartRow {
    StartRow {
        position: [x, y, 0.0],
        team,
    }
}

fn census() -> Census {
    Census {
        flags: vec![
            flag(0, 0, 2.0),
            flag(8, 1, 40.0),
            flag(0, 1, 26.0),
            flag(12, 0, 14.0),
            flag(11, 0, 15.0),
            flag(8, 0, 6.0),
        ],
        starts: vec![start(0, 0.0, 0.0), start(0, 4.0, 2.0), start(1, 30.0, 8.0)],
        items: vec![
            Item {
                name: r"weapons\rifle\sniper_rifle\sniper_rifle".into(),
                position: [9.0, 9.0, 0.0],
            },
            Item {
                name: r"weapons\pistol\magnum\magnum".into(),
                position: [1.0, 1.0, 0.0],
            },
            Item {
                name: r"weapons\rifle\sniper_rifle\sniper_rifle".into(),
                position: [19.0, 9.0, 0.0],
            },
        ],
    }
}

#[test]
fn an_anchor_resolves_from_the_maps_own_flags() {
    let c = census();
    let flag = |kind, team, nth| Anchor::Flag { kind, team, nth };
    // A flag by kind and team, in row order.
    assert_eq!(
        flag(FlagKind::CtfStand, Some(Team::Blue), 0).resolve(&c),
        Ok([26.0, 0.0, 1.0])
    );
    assert_eq!(
        flag(FlagKind::TeleporterExit, Some(Team::Red), 0).resolve(&c),
        Ok([6.0, 0.0, 1.0])
    );
    // With no team, the nth of the kind across teams; a hill is any
    // corner, in row order.
    assert_eq!(
        flag(FlagKind::CtfStand, None, 1).resolve(&c),
        Ok([26.0, 0.0, 1.0])
    );
    assert_eq!(
        flag(FlagKind::Hill, None, 1).resolve(&c),
        Ok([15.0, 0.0, 1.0])
    );
    // The mean of a team's starts, and the nth pad of a name suffix.
    assert_eq!(
        Anchor::Starts { team: Team::Red }.resolve(&c),
        Ok([2.0, 1.0, 0.0])
    );
    assert_eq!(
        Anchor::Item {
            suffix: "sniper_rifle",
            nth: 1
        }
        .resolve(&c),
        Ok([19.0, 9.0, 0.0])
    );
    // Between and the fallback.
    const RED: Anchor = Anchor::Flag {
        kind: FlagKind::CtfStand,
        team: Some(Team::Red),
        nth: 0,
    };
    const BLUE: Anchor = Anchor::Flag {
        kind: FlagKind::CtfStand,
        team: Some(Team::Blue),
        nth: 0,
    };
    const TERRITORY: Anchor = Anchor::Flag {
        kind: FlagKind::Territory,
        team: None,
        nth: 0,
    };
    assert_eq!(
        Anchor::Between(&RED, &BLUE, 0.5).resolve(&c),
        Ok([14.0, 0.0, 1.0])
    );
    assert_eq!(
        Anchor::Or(&TERRITORY, &RED).resolve(&c),
        Ok([2.0, 0.0, 1.0])
    );
    // What the map lacks is named.
    let err = TERRITORY.resolve(&c).unwrap_err();
    assert!(err.contains("territory flag #0"), "{err}");
    let err = flag(FlagKind::CtfStand, Some(Team::Red), 1)
        .resolve(&c)
        .unwrap_err();
    assert!(err.contains("ctf flag #1 of team 0"), "{err}");
    assert!(
        Anchor::Item {
            suffix: "rocket_launcher",
            nth: 0
        }
        .resolve(&c)
        .is_err()
    );
    assert!(
        Anchor::Starts { team: Team::Blue }
            .resolve(&Census::default())
            .is_err()
    );
}

#[test]
fn a_layout_resolves_each_drop_and_names_the_ones_it_cannot() {
    static LAYOUT: Layout = Layout {
        map: "court",
        home: Team::Red,
        two_teams: true,
        base: BaseSpec {
            anchor: Anchor::Flag {
                kind: FlagKind::CtfStand,
                team: Some(Team::Red),
                nth: 0,
            },
            positions: 8,
        },
        drops: &[
            DropSpec {
                name: "hill",
                anchor: Anchor::Starts { team: Team::Red },
                offset: [1.0, 0.0, 0.0],
                radius: 4.0,
                roles: Roles::LINE,
                sides: Sides::Both,
            },
            DropSpec {
                name: "west_cave",
                anchor: Anchor::Item {
                    suffix: "rocket_launcher",
                    nth: 0,
                },
                offset: [0.0; 3],
                radius: 4.0,
                roles: Roles::FLANK,
                sides: Sides::Both,
            },
        ],
        cover: &[],
    };
    let r = LAYOUT.resolve(&census());
    assert_eq!(r.base_point, Some([2.0, 0.0, 1.0]));
    assert_eq!(r.drops.len(), 1);
    assert_eq!(r.drops[0].point, [3.0, 1.0, 0.0]);
    assert_eq!(r.drops[0].roles, firefight::roles::LINE);
    assert_eq!(r.refused.len(), 1);
    assert!(
        r.refused[0].starts_with("drop 'west_cave': the map has no weapon #0 named '*rocket_launcher'"),
        "{:?}",
        r.refused
    );
}

#[test]
fn builtin_layouts_have_six_drops_a_side() {
    assert_eq!(ALL.len(), 2);
    for layout in ALL {
        assert!(for_map(layout.map).is_some());
        assert!(
            layout.drops.len() <= 12,
            "{}: more drops than a kit holds",
            layout.map
        );
        let mut names: Vec<_> = layout.drops.iter().map(|d| d.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), layout.drops.len(), "{}: drop names", layout.map);
        for d in layout.drops {
            assert_ne!(d.roles.0, 0, "{}: {} has no role", layout.map, d.name);
            assert!(d.radius > 0.0 && d.radius.is_finite());
            assert!(d.offset.iter().all(|x| x.is_finite()));
        }
        for &team in layout.served() {
            let seen = layout.drops.iter().filter(|d| d.sides.serves(team)).count();
            assert!(
                seen >= 6,
                "{}: team {} meets {seen} drops",
                layout.map,
                team.index()
            );
        }
        // Every side meets a line squad and the boss wave has a heavy zone.
        assert!(layout.drops.iter().any(|d| d.roles.has(Roles::LINE)));
        assert!(layout.drops.iter().any(|d| d.roles.has(Roles::HEAVY)));
    }
    // Coagulation serves both teams, Zanzibar one.
    assert_eq!(for_map("Coagulation").unwrap().served().len(), 2);
    assert_eq!(for_map("zanzibar").unwrap().served().len(), 1);
    assert!(for_map("lockout").is_none());
}

#[test]
fn the_digest_follows_what_the_layout_says() {
    let a = coagulation::LAYOUT;
    let mut b = a;
    b.base.positions = 7;
    assert_eq!(a.digest(), coagulation::LAYOUT.digest());
    assert_ne!(a.digest(), b.digest());
    assert_ne!(a.digest(), zanzibar::LAYOUT.digest());
}
