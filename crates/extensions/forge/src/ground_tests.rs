//! AI ground in a session: markers, the stale rule, build on save and the
//! recipe in the file. Invented block, tags and builder; no game.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use forge_doc::{
    BuilderVersions, Edit, ForgeDoc, GroundClass, Limits, MapRef, Placement, Recipe, Sha256,
    WorldBounds,
};

/// A world that counts what a session asks of it.
#[derive(Default)]
struct Counting {
    calls: usize,
    next: u32,
}

impl World for Counting {
    fn place(&mut self, _: &PaletteItem, _: Frame) -> Result<u32, String> {
        self.calls += 1;
        self.next += 1;
        Ok(self.next)
    }
    fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.place(item, frame)
    }
    fn move_object(&mut self, _: u32, _: Frame) -> Result<(), String> {
        self.calls += 1;
        Ok(())
    }
    fn delete(&mut self, _: u32) -> Result<(), String> {
        self.calls += 1;
        Ok(())
    }
}

/// A builder that records the documents it was given. Its disc carries
/// every class on one invented donor, `outskirts`; with `room` set the map
/// keeps that many classes and cuts the rest from the end.
#[derive(Default)]
struct Kit {
    built: Vec<ForgeDoc>,
    refuse: bool,
    room: Option<usize>,
    /// Donor choices it was asked for.
    asked: Vec<Vec<String>>,
}

fn recipe() -> GroundRecipe {
    GroundRecipe {
        kit_version: 3,
        home: 0,
        probe: false,
        classes: vec![],
    }
}

/// The invented donor every class of [`Kit`] comes from.
fn outskirts() -> MapRef {
    MapRef {
        map: "outskirts".into(),
        sha256: Sha256::of(b"an invented campaign map"),
    }
}

impl GroundBuilder for Kit {
    fn recipe(&self) -> GroundRecipe {
        recipe()
    }

    fn classes(&mut self, names: &[String]) -> Result<Vec<GroundClass>, String> {
        self.asked.push(names.to_vec());
        Ok(names
            .iter()
            .map(|name| GroundClass {
                name: name.clone(),
                donor: outskirts(),
            })
            .collect())
    }

    fn build(&mut self, play: &ForgeDoc) -> Result<GroundReport, String> {
        if self.refuse {
            return Err("No room: the sectors take 9 KB too many.".into());
        }
        self.built.push(play.clone());
        let names = play
            .ground_recipe()
            .ok()
            .flatten()
            .map(|r| r.class_names())
            .unwrap_or_default();
        let keep = self.room.unwrap_or(names.len()).min(names.len());
        Ok(GroundReport {
            zones: 2,
            squads: 16,
            classes: std::iter::once("elite".to_owned())
                .chain(names[..keep].iter().cloned())
                .collect(),
            cut: names[keep..].iter().rev().cloned().collect(),
            ..GroundReport::default()
        })
    }
}

const CRATE: usize = 0;
const WEAPON: usize = 1;

fn limits() -> Limits {
    Limits {
        base_counts: BTreeMap::from([
            (BlockKind::Crates, 2),
            (BlockKind::Weapons, 2),
            (BlockKind::NetgameFlags, 3),
        ]),
        world: Some(WorldBounds {
            min: [-30.; 3],
            max: [30.; 3],
        }),
        ..Default::default()
    }
}

fn session() -> Session {
    let palette = vec![
        PaletteItem {
            block: BlockKind::Crates,
            tag: TagKey {
                group: "bloc".into(),
                path: r"fixture\cube".into(),
            },
        },
        PaletteItem {
            block: BlockKind::Weapons,
            tag: TagKey {
                group: "weap".into(),
                path: r"fixture\rifle".into(),
            },
        },
    ];
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "court".into(),
            sha256: Sha256::of(b"an invented court"),
        },
        sources: vec![],
        title: "Waves".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    };
    Session::new(base, limits(), palette, 16).unwrap()
}

fn at(x: f32, yaw: f32) -> Frame {
    Frame {
        position: [x, 4., 0.],
        rotation: [yaw, 0., 0.],
    }
}

/// Where the store keeps the variant `title` of the fixture's map.
fn variant_file(dir: &std::path::Path, title: &str) -> PathBuf {
    dir.join("map-variants/court")
        .join(format!("{title}.h2mapvariant"))
}

fn temp_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "forge-ground-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[test]
fn a_marker_is_a_labelled_flag_and_places_nothing_in_the_world() {
    let mut session = session();
    let mut world = Counting::default();
    let id = session
        .place_marker(&mut world, 1, GroundLabel::Drop, at(5., 0.5), 0, None)
        .unwrap();
    let line = session
        .place_marker(&mut world, 2, GroundLabel::Line, at(-5., 1.), 1, Some(2))
        .unwrap();
    assert_eq!(world.calls, 0, "a marker is document data");

    let markers = session.markers().unwrap();
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].id, id);
    assert_eq!(markers[0].label, GroundLabel::Drop);
    assert_eq!(markers[0].frame, at(5., 0.5));
    assert_eq!(markers[1].id, line);
    assert_eq!((markers[1].number, markers[1].team), (Some(2), 1));

    // The palette's counts, in label order.
    assert_eq!(session.ground_counts(), [1, 0, 0, 0, 0, 1, 0]);
    // The document says what the build reads: flags 3 and 4 after the
    // map's three.
    let marks = session.ground_marks().unwrap();
    let got: Vec<_> = marks.iter().map(|m| (m.kind, m.index)).collect();
    assert_eq!(got, [(GroundLabel::Drop, 3), (GroundLabel::Line, 4)]);
}

#[test]
fn a_marker_moves_turns_and_is_removed_with_its_label() {
    let mut session = session();
    let mut world = Counting::default();
    let id = session
        .place_marker(&mut world, 1, GroundLabel::Cover, at(5., 0.), 0, None)
        .unwrap();
    session.move_marker(&mut world, 2, id, at(8., 1.)).unwrap();
    let marker = session.markers().unwrap().remove(0);
    assert_eq!(marker.frame, at(8., 1.));
    // A move that changes nothing writes nothing.
    let ops = session.document().ops.len();
    session.move_marker(&mut world, 3, id, at(8., 1.)).unwrap();
    assert_eq!(session.document().ops.len(), ops);

    session.undo(&mut world, 4).unwrap();
    assert_eq!(session.markers().unwrap()[0].frame, at(5., 0.));
    session.remove_marker(&mut world, 5, id).unwrap();
    assert!(session.markers().unwrap().is_empty());
    assert_eq!(session.ground_counts(), [0; 7]);
    assert_eq!(world.calls, 0);
}

#[test]
fn a_marker_out_of_bounds_or_with_a_bad_team_is_refused() {
    let mut session = session();
    let mut world = Counting::default();
    assert!(
        session
            .place_marker(&mut world, 1, GroundLabel::Drop, at(99., 0.), 0, None)
            .is_err()
    );
    assert!(
        session
            .place_marker(&mut world, 1, GroundLabel::Base, at(1., 0.), 2, None)
            .is_err()
    );
    assert!(session.document().ops.is_empty());
}

#[test]
fn ground_is_not_built_until_asked_and_then_follows_the_document() {
    let mut session = session();
    let mut world = Counting::default();
    let mut kit = Kit::default();
    let id = session
        .place_marker(&mut world, 1, GroundLabel::Drop, at(5., 0.), 0, None)
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::NotBuilt);
    assert_eq!(session.ground_recipe(), None);
    assert!(kit.built.is_empty(), "placing builds nothing");

    let report = session.build_ground(&mut kit).unwrap().clone();
    assert_eq!((report.zones, report.squads), (2, 16));
    assert_eq!(session.ground_state(), GroundState::Current);
    assert_eq!(session.ground_recipe(), Some(recipe()));
    assert_eq!(session.ground_report(), Some(&report));

    // The build sees the play form with the recipe.
    let seen = &kit.built[0];
    assert_eq!(seen.ground_recipe().unwrap(), Some(recipe()));
    assert!(!seen.recipes.contains(&forge_doc::kit()));

    // A mark that moves makes it stale; building again makes it current.
    session.move_marker(&mut world, 2, id, at(6., 0.)).unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
    assert!(session.ensure_ground(&mut kit).unwrap());
    assert_eq!(session.ground_state(), GroundState::Current);
    assert!(!session.ensure_ground(&mut kit).unwrap(), "already current");
    assert_eq!(kit.built.len(), 2);

    // Undo is a change too.
    session.undo(&mut world, 3).unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
}

#[test]
fn something_solid_marks_it_stale_and_anything_else_does_not() {
    let mut session = session();
    let mut world = Counting::default();
    let mut kit = Kit::default();
    session.build_ground(&mut kit).unwrap();
    assert_eq!(session.ground_state(), GroundState::Current);

    // A weapon has no collision the ground is built over.
    let weapon = session.place(&mut world, 1, WEAPON, at(1., 0.)).unwrap();
    session
        .move_object(&mut world, 2, weapon, at(2., 0.))
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::Current);

    // A crate does.
    let wall = session.place(&mut world, 3, CRATE, at(3., 0.)).unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
    session.build_ground(&mut kit).unwrap();
    session
        .move_object(&mut world, 4, wall, at(4., 0.))
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
    session.build_ground(&mut kit).unwrap();
    session.delete(&mut world, 5, wall).unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
}

#[test]
fn a_build_that_fails_changes_nothing() {
    let mut session = session();
    let mut kit = Kit {
        refuse: true,
        ..Kit::default()
    };
    let error = session.build_ground(&mut kit).unwrap_err();
    assert!(error.contains("No room"), "{error}");
    assert_eq!(session.ground_state(), GroundState::NotBuilt);
    assert_eq!(
        session.ground_recipe(),
        None,
        "no recipe from a failed first build"
    );

    kit.refuse = false;
    session.build_ground(&mut kit).unwrap();
    let mut world = Counting::default();
    session
        .place_marker(&mut world, 1, GroundLabel::Drop, at(1., 0.), 0, None)
        .unwrap();
    kit.refuse = true;
    assert!(session.ensure_ground(&mut kit).is_err());
    assert_eq!(session.ground_state(), GroundState::Stale);
    assert_eq!(session.ground_recipe(), Some(recipe()));
}

#[test]
fn a_save_with_stale_ground_builds_first_and_one_with_none_does_not() {
    let dir = temp_dir();
    let mut session = session();
    let mut world = Counting::default();
    let mut kit = Kit::default();

    // Ground never asked for: saved as it is, nothing built.
    session
        .place_marker(&mut world, 1, GroundLabel::Drop, at(1., 0.), 0, None)
        .unwrap();
    session
        .save_building(&dir, "Plain", false, &mut kit)
        .unwrap();
    assert!(kit.built.is_empty());
    let plain = forge_doc::ForgeDoc::from_bytes(
        &fs::read(variant_file(&dir, "Plain")).unwrap(),
        &limits(),
    )
    .unwrap();
    assert_eq!(plain.ground_recipe().unwrap(), None);

    // Built, then changed: the save builds first.
    session.build_ground(&mut kit).unwrap();
    session
        .place_marker(&mut world, 2, GroundLabel::Cover, at(2., 0.), 0, None)
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
    session
        .save_building(&dir, "Waves", false, &mut kit)
        .unwrap();
    assert_eq!(kit.built.len(), 2);
    assert_eq!(session.ground_state(), GroundState::Current);
    let saved = forge_doc::ForgeDoc::from_bytes(
        &fs::read(variant_file(&dir, "Waves")).unwrap(),
        &limits(),
    )
    .unwrap();
    assert_eq!(saved.ground_recipe().unwrap(), Some(recipe()));
    assert_eq!(saved.tier(), forge_doc::Tier::Variant);

    // A build that fails stops the save.
    session
        .place_marker(&mut world, 3, GroundLabel::Site, at(3., 0.), 0, None)
        .unwrap();
    kit.refuse = true;
    let error = session
        .save_building(&dir, "Waves", true, &mut kit)
        .unwrap_err()
        .to_string();
    assert!(error.contains("nothing was saved"), "{error}");
    let still = fs::read(variant_file(&dir, "Waves")).unwrap();
    assert_eq!(
        forge_doc::ForgeDoc::from_bytes(&still, &limits()).unwrap(),
        saved
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_load_takes_the_recipe_back_out_and_builds_nothing() {
    let dir = temp_dir();
    let mut first = session();
    let mut world = Counting::default();
    let mut kit = Kit::default();
    first
        .place_marker(&mut world, 1, GroundLabel::Goal, at(1., 0.), 0, None)
        .unwrap();
    first.build_ground(&mut kit).unwrap();
    let stem = first.save_variant(&dir, "Stand", false).unwrap();

    let mut second = session();
    let mut world = Counting::default();
    let doc = files::variants::load(
        &dir,
        "court",
        &stem,
        Some(&Sha256::of(b"an invented court")),
    )
    .unwrap();
    second.load_variant(&mut world, 1, doc).unwrap();
    assert_eq!(second.ground_recipe(), Some(recipe()));
    assert_eq!(second.ground_state(), GroundState::NotBuilt);
    assert_eq!(second.markers().unwrap().len(), 1);
    // The document the log chains from has no ground recipe in it.
    assert!(second.document().ground_recipe().unwrap().is_none());
    assert_eq!(second.document().ops, first.document().ops);
    // Its ground form is what was saved.
    assert_eq!(
        second.ground_form().ground_recipe().unwrap(),
        Some(recipe())
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_play_form_a_build_sees_is_a_map_variant() {
    let mut session = session();
    let mut world = Counting::default();
    let mut kit = Kit::default();
    session
        .place_marker(&mut world, 1, GroundLabel::Drop, at(1., 0.), 0, None)
        .unwrap();
    session.build_ground(&mut kit).unwrap();
    let play = &kit.built[0];
    assert_eq!(play.tier(), forge_doc::Tier::Variant);
    assert!(play.check_variant().is_ok());
    assert!(matches!(
        play.recipes[..],
        [Recipe::Generate { ref generator, .. }] if generator == forge_doc::GROUND_RECIPE
    ));
    // Neither the placement nor the recipe is other than a variant's.
    assert!(
        play.ops
            .iter()
            .any(|op| matches!(op.edit, Edit::Placement(Placement::Add { .. })))
    );
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn a_class_change_marks_the_ground_stale() {
    let mut session = session();
    let mut kit = Kit::default();
    session.build_ground(&mut kit).unwrap();
    assert_eq!(session.ground_state(), GroundState::Current);
    assert_eq!(session.ground_classes(), Vec::<String>::new());

    // Asking for classes marks it stale; the list is kept in keep order.
    session
        .set_ground_classes(&names(&["hunter", "grunt"]))
        .unwrap();
    assert_eq!(session.ground_classes(), names(&["grunt", "hunter"]));
    assert_eq!(session.ground_state(), GroundState::Stale);
    assert!(kit.asked.is_empty(), "donors are chosen at the build");

    // The build asks the builder for the donors and puts them in the recipe.
    assert!(session.ensure_ground(&mut kit).unwrap());
    assert_eq!(kit.asked, [names(&["grunt", "hunter"])]);
    assert_eq!(session.ground_state(), GroundState::Current);
    let recipe = session.ground_recipe().unwrap();
    assert_eq!(recipe.class_names(), names(&["grunt", "hunter"]));
    assert!(recipe.classes.iter().all(|c| c.donor == outskirts()));
    let seen = kit.built.last().unwrap().ground_recipe().unwrap().unwrap();
    assert_eq!(seen, recipe);
    assert_eq!(
        session.ground_report().unwrap().classes,
        names(&["elite", "grunt", "hunter"])
    );

    // The same list again changes nothing; another marks it stale.
    session
        .set_ground_classes(&names(&["grunt", "hunter"]))
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::Current);
    session.set_ground_classes(&names(&["grunt"])).unwrap();
    assert_eq!(session.ground_state(), GroundState::Stale);
    // Going back to the built list makes it current again.
    session
        .set_ground_classes(&names(&["grunt", "hunter"]))
        .unwrap();
    assert_eq!(session.ground_state(), GroundState::Current);

    // A name the table lacks, or one given twice, changes nothing.
    for bad in [names(&["elite"]), names(&["marine"]), names(&["grunt", "grunt"])] {
        assert!(session.set_ground_classes(&bad).is_err(), "{bad:?}");
        assert_eq!(session.ground_state(), GroundState::Current);
    }
}

#[test]
fn a_failed_class_build_keeps_the_list_asked_for() {
    let mut session = session();
    let mut kit = Kit::default();
    session.build_ground(&mut kit).unwrap();
    session.set_ground_classes(&names(&["jackal"])).unwrap();
    kit.refuse = true;
    assert!(session.build_ground(&mut kit).is_err());
    assert_eq!(session.ground_classes(), names(&["jackal"]));
    assert!(session.ground_recipe().unwrap().classes.is_empty());
    assert_eq!(session.ground_state(), GroundState::Stale);

    // A builder that copies no classes refuses the list by name.
    struct Plain;
    impl GroundBuilder for Plain {
        fn recipe(&self) -> GroundRecipe {
            recipe()
        }
        fn build(&mut self, _: &ForgeDoc) -> Result<GroundReport, String> {
            Ok(GroundReport::default())
        }
    }
    assert_eq!(
        session.build_ground(&mut Plain).unwrap_err(),
        crate::NO_CLASSES
    );
    assert_eq!(session.ground_classes(), names(&["jackal"]));
}

#[test]
fn wave_problems_name_the_classes_the_ground_built() {
    let mut session = session();
    let mut kit = Kit::default();
    let mut def = forge_doc::waves::default_set();
    def.rounds[0].waves[0].squads[0].class = "grunt".into();
    def.scoring.by_class.insert("grunt".into(), 100);
    session.set_waves(Some(def)).unwrap();
    let grunts = |s: &Session| {
        s.wave_problems()
            .iter()
            .filter(|p| p.what == forge_doc::waves::ProblemKind::ClassNotBuilt)
            .count()
    };
    // No grunt asked for: the one grunt squad is a problem.
    assert_eq!(grunts(&session), 1);
    // Asked for, before a build: the recipe's list counts.
    session.set_ground_classes(&names(&["grunt"])).unwrap();
    assert_eq!(grunts(&session), 0);
    // Built: the build's classes count.
    session.build_ground(&mut kit).unwrap();
    assert_eq!(grunts(&session), 0);
    // Cut for headroom: a problem again.
    session
        .set_ground_classes(&names(&["grunt", "jackal"]))
        .unwrap();
    kit.room = Some(0);
    session.build_ground(&mut kit).unwrap();
    assert_eq!(
        session.ground_report().unwrap().cut,
        names(&["jackal", "grunt"])
    );
    assert_eq!(grunts(&session), 1);
}
