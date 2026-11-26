//! The wave set in a session: zones that follow their markers, the drop
//! cap, the validator's refusals, the save and load forms and the
//! problems. Invented map, markers and wave sets; no game, no disc.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use forge_doc::waves::{self, DROPS_FULL, Def, Drop as DropKind, MAX_DROPS, Problem, ProblemKind};
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, Sha256, WorldBounds};
use serde_json::{Value as Json, json};

/// A world that places nothing: markers are document data.
#[derive(Default)]
struct Still {
    next: u32,
}

impl World for Still {
    fn place(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
        self.next += 1;
        Ok(self.next)
    }
    fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.place(item, frame)
    }
    fn move_object(&mut self, _: u32, _: Frame) -> Result<(), String> {
        Ok(())
    }
    fn delete(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
}

fn limits() -> Limits {
    Limits {
        base_counts: BTreeMap::from([(BlockKind::NetgameFlags, 2)]),
        world: Some(WorldBounds {
            min: [-50.; 3],
            max: [50.; 3],
        }),
        ..Default::default()
    }
}

fn session() -> Session {
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "yard".into(),
            sha256: Sha256::of(b"an invented yard"),
        },
        sources: vec![],
        title: "Yard".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    };
    Session::new(base, limits(), vec![], 16).unwrap()
}

fn at(x: f32) -> Frame {
    Frame {
        position: [x, 2., 0.],
        rotation: [0., 0., 0.],
    }
}

fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "forge-waves-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// An invented set of one round: each wave has one squad of `class` for
/// each of `drops` (`"edge"` or `{"prefer_zone": n}`).
fn set(name: &str, class: &str, drops: &[Json]) -> Def {
    let squads: Vec<Json> = drops
        .iter()
        .map(|drop| {
            json!({
                "class": class,
                "count": {"per_player": 1, "min": 1, "max": 2},
                "drop": drop,
                "weapon_tier": 0,
            })
        })
        .collect();
    let waves: Vec<Json> = (0..5)
        .map(|i| json!({"squads": squads, "boss": i == 4, "gap_ticks": 300}))
        .collect();
    let mut by_class = serde_json::Map::new();
    by_class.insert(class.into(), json!(100));
    serde_json::from_value(json!({
        "format": 3,
        "name": name,
        "teams": 1,
        "rounds": [{"waves": waves, "skulls_on": []}],
        "set": {"endless": false, "scale": 0, "skulls": []},
        "lives": {"pool_start": 5, "per_round_bonus": 1, "shared": true},
        "caps": {"live_actors_by_players": [[4, 32]]},
        "scoring": {
            "by_class": by_class,
            "player_kill": 0,
            "difficulty_multiplier": {"easy": 1000, "normal": 1000, "heroic": 1000, "legendary": 1000},
            "skull_multipliers": {},
            "assist": 500,
        },
    }))
    .unwrap()
}

fn zone(n: u8) -> Json {
    json!({"prefer_zone": n})
}

/// The first wave's squads' drops.
fn drops_of(session: &Session) -> Vec<DropKind> {
    session.waves().unwrap().rounds[0].waves[0]
        .squads
        .iter()
        .map(|s| s.drop)
        .collect()
}

fn drop_marker(session: &mut Session, world: &mut Still, tick: u32, x: f32) -> OpId {
    session
        .place_marker(world, tick, GroundLabel::Drop, at(x), 0, None)
        .unwrap()
}

#[test]
fn a_moved_drop_keeps_its_squads() {
    let mut session = session();
    let mut world = Still::default();
    let west = drop_marker(&mut session, &mut world, 1, -10.);
    let east = drop_marker(&mut session, &mut world, 2, 10.);
    assert_eq!(session.drops(), [west, east]);
    session
        .set_waves(Some(set(
            "Yard",
            "elite",
            &[zone(0), zone(1), json!("edge")],
        )))
        .unwrap();

    // West moves past east: it is zone 1 now, and its squad follows it.
    session.move_marker(&mut world, 3, west, at(20.)).unwrap();
    assert_eq!(session.drops(), [east, west]);
    assert_eq!(
        drops_of(&session),
        [
            DropKind::PreferZone(1),
            DropKind::PreferZone(0),
            DropKind::Edge
        ]
    );
    // Every wave is renumbered, not only the first.
    for wave in &session.waves().unwrap().rounds[0].waves {
        assert_eq!(wave.squads[0].drop, DropKind::PreferZone(1));
    }

    // A move that keeps the order changes nothing.
    let before = session.waves().cloned();
    session.move_marker(&mut world, 4, west, at(30.)).unwrap();
    assert_eq!(session.waves().cloned(), before);
}

#[test]
fn a_removed_drop_sends_its_squads_to_the_edge() {
    let mut session = session();
    let mut world = Still::default();
    let west = drop_marker(&mut session, &mut world, 1, -10.);
    drop_marker(&mut session, &mut world, 2, 0.);
    drop_marker(&mut session, &mut world, 3, 10.);
    session
        .set_waves(Some(set("Yard", "elite", &[zone(0), zone(1), zone(2)])))
        .unwrap();

    session.remove_marker(&mut world, 4, west).unwrap();
    assert_eq!(
        drops_of(&session),
        [
            DropKind::Edge,
            DropKind::PreferZone(0),
            DropKind::PreferZone(1)
        ]
    );

    // An added drop renumbers the squads past it, too.
    drop_marker(&mut session, &mut world, 5, -20.);
    assert_eq!(
        drops_of(&session),
        [
            DropKind::Edge,
            DropKind::PreferZone(1),
            DropKind::PreferZone(2)
        ]
    );
}

#[test]
fn undo_of_a_drop_move_remaps_back() {
    let mut session = session();
    let mut world = Still::default();
    let west = drop_marker(&mut session, &mut world, 1, -10.);
    drop_marker(&mut session, &mut world, 2, 10.);
    let wanted = set("Yard", "elite", &[zone(0), zone(1)]);
    session.set_waves(Some(wanted.clone())).unwrap();

    session.move_marker(&mut world, 3, west, at(20.)).unwrap();
    assert_ne!(session.waves(), Some(&wanted));
    session.undo(&mut world, 4).unwrap();
    assert_eq!(session.waves(), Some(&wanted));
    session.redo(&mut world, 5).unwrap();
    assert_eq!(
        drops_of(&session),
        [DropKind::PreferZone(1), DropKind::PreferZone(0)]
    );
}

#[test]
fn a_thirteenth_drop_is_refused() {
    let mut session = session();
    let mut world = Still::default();
    for i in 0..MAX_DROPS {
        drop_marker(&mut session, &mut world, i as u32 + 1, i as f32);
    }
    assert_eq!(session.drops().len(), MAX_DROPS);
    let ops = session.document().ops.clone();
    let error = session
        .place_marker(&mut world, 99, GroundLabel::Drop, at(-30.), 0, None)
        .unwrap_err();
    assert_eq!(error, DROPS_FULL);
    assert_eq!(session.document().ops, ops, "nothing was placed");
    // Other markers still place.
    session
        .place_marker(&mut world, 100, GroundLabel::Cover, at(-30.), 0, None)
        .unwrap();
}

#[test]
fn a_refused_set_changes_nothing() {
    let mut session = session();
    let kept = set("Kept", "elite", &[json!("edge")]);
    session.set_waves(Some(kept.clone())).unwrap();

    // A class no Forge map may name.
    let error = session
        .set_waves(Some(set("Ogres", "ogre", &[json!("edge")])))
        .unwrap_err();
    assert!(error.to_string().contains("ogre"), "{error}");
    assert_eq!(session.waves(), Some(&kept));

    // A wave over the enemy limit: the validator's sentence.
    let mut over = kept.clone();
    over.caps.live_actors_by_players = vec![(4, 1)];
    let error = session.set_waves(Some(over)).unwrap_err().to_string();
    assert!(error.contains("round 1, wave 1 needs"), "{error}");
    assert_eq!(session.waves(), Some(&kept));

    // Clearing returns to Holdout.
    session.set_waves(None).unwrap();
    assert!(session.waves_are_default());
    assert_eq!(session.waves_or_default(), waves::default_set());
    assert_eq!(session.waves_or_default().name, "Holdout");
}

#[test]
fn a_save_writes_the_set_and_a_load_takes_it_out() {
    let dir = temp_dir();
    let mut first = session();
    let mut world = Still::default();
    drop_marker(&mut first, &mut world, 1, 0.);
    let saved = set("Yard", "elite", &[zone(0)]);
    first.set_waves(Some(saved.clone())).unwrap();
    // The set is beside the document, not in it.
    assert_eq!(first.document().waves, None);
    let stem = first.save_variant(&dir, "Yard", false).unwrap();

    let file = dir
        .join("map-variants/yard")
        .join(format!("{stem}.h2mapvariant"));
    let doc = ForgeDoc::from_bytes(&fs::read(&file).unwrap(), &limits()).unwrap();
    assert_eq!(doc.waves, Some(saved.clone()));
    assert_eq!(doc.format, forge_doc::FORMAT_WAVES);
    assert_eq!(doc.tier(), forge_doc::Tier::Variant);

    let mut second = session();
    let mut world = Still::default();
    let loaded =
        files::variants::load(&dir, "yard", &stem, Some(&Sha256::of(b"an invented yard"))).unwrap();
    second.load_variant(&mut world, 1, loaded).unwrap();
    assert_eq!(second.waves(), Some(&saved));
    assert_eq!(second.document().waves, None);
    assert_eq!(
        second.document().format,
        second.document().required_format()
    );
    assert_eq!(second.document().ops, first.document().ops);

    // Reset waves returns to the set as loaded.
    second.set_waves(None).unwrap();
    second.reset_waves();
    assert_eq!(second.waves(), Some(&saved));

    // A session with no set saves none.
    let plain = session().variant_form("Plain");
    assert_eq!(plain.waves, None);
    assert!(!plain.canonical_json().unwrap().contains("\"waves\""));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn problems_name_a_class_the_ground_lacks() {
    let mut session = session();
    let mut world = Still::default();
    drop_marker(&mut session, &mut world, 1, 0.);
    // Holdout asks for nothing the stage-one ground lacks.
    assert_eq!(session.wave_problems(), []);

    session
        .set_waves(Some(set("Grunts", "grunt", &[zone(0), zone(3)])))
        .unwrap();
    let problems = session.wave_problems();
    let first: Vec<Problem> = problems.iter().copied().take(3).collect();
    assert_eq!(
        first,
        [
            Problem {
                round: 1,
                wave: 1,
                squad: 1,
                what: ProblemKind::ClassNotBuilt
            },
            Problem {
                round: 1,
                wave: 1,
                squad: 2,
                what: ProblemKind::ClassNotBuilt
            },
            Problem {
                round: 1,
                wave: 1,
                squad: 2,
                what: ProblemKind::ZoneNotPlaced
            },
        ]
    );
    // Three for each of the five waves.
    assert_eq!(problems.len(), 15);
    let def = session.waves_or_default();
    assert_eq!(
        problems[0].sentence(&def),
        "Round 1, wave 1, squad 1: the AI ground has no grunt."
    );
    assert_eq!(
        problems[2].sentence(&def),
        "Round 1, wave 1, squad 2: no wave drop is zone 3."
    );
}

#[test]
fn only_the_host_seat_edits_the_waves() {
    let mut guest = session().share(crate::net::Seat(1), "Guest");
    let error = guest
        .set_waves(Some(set("Yard", "elite", &[json!("edge")])))
        .unwrap_err();
    assert!(
        error.to_string().contains(crate::WAVES_HOST_ONLY),
        "{error}"
    );
    assert!(guest.waves().is_none());
    let mut host = session().share(crate::net::HOST_SEAT, "Host");
    host.set_waves(Some(set("Yard", "elite", &[json!("edge")])))
        .unwrap();
}

/// Reset waves returns to the set as loaded, and the session says whether
/// its set was edited since: what Start from asks on and the drop count
/// the Drop picker lists (docs/game/forge-firefight-ui.md, "The Waves
/// screens").
#[test]
fn reset_returns_to_the_loaded_set() {
    let dir = temp_dir();
    let mut first = session();
    let mut world = Still::default();
    drop_marker(&mut first, &mut world, 1, 0.);
    drop_marker(&mut first, &mut world, 2, 4.);
    assert_eq!(first.wave_drops(), 2);
    assert!(!first.waves_edited(), "no set, none loaded");
    let saved = set("Yard", "elite", &[zone(1)]);
    first.set_waves(Some(saved.clone())).unwrap();
    assert!(first.waves_edited());
    let stem = first.save_variant(&dir, "Yard", false).unwrap();

    let mut second = session();
    let mut world = Still::default();
    let loaded =
        files::variants::load(&dir, "yard", &stem, Some(&Sha256::of(b"an invented yard"))).unwrap();
    second.load_variant(&mut world, 1, loaded).unwrap();
    assert_eq!(second.waves_loaded(), Some(&saved));
    assert!(!second.waves_edited(), "as loaded");

    second
        .set_waves(Some(set("Other", "elite", &[json!("edge")])))
        .unwrap();
    assert!(second.waves_edited());
    second.reset_waves();
    assert_eq!(second.waves(), Some(&saved));
    assert!(!second.waves_edited());
    fs::remove_dir_all(dir).unwrap();
}
