//! **Generate nav** in a session: the generator is asked for the play
//! form with the map's counts, its report is kept, an edit makes it
//! stale, a failure keeps the last one, and a load starts again.
//! Invented block, tags and generator; no game.
use std::collections::BTreeMap;

use super::*;
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, Sha256, WorldBounds};

#[derive(Default)]
struct Counting {
    next: u32,
}

impl World for Counting {
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

/// A generator that records what it was given.
#[derive(Default)]
struct Gen {
    seen: Vec<(ForgeDoc, BTreeMap<BlockKind, u32>)>,
    refuse: bool,
}

fn report() -> NavReport {
    NavReport {
        sectors: 12,
        links: BTreeMap::from([("jump".to_string(), 2)]),
        out_of_map: 1,
        solids: 1,
        problems: vec![NavProblem {
            kind: "start_off_mesh".into(),
            what: "start 0".into(),
            position: [1., 2., 3.],
            reason: "no floor under it".into(),
        }],
        lines: vec![NavLine {
            kind: "sector".into(),
            a: [0.; 3],
            b: [1., 0., 0.],
        }],
    }
}

impl NavBuilder for Gen {
    fn generate(
        &mut self,
        play: &ForgeDoc,
        base_counts: &BTreeMap<BlockKind, u32>,
    ) -> Result<NavReport, String> {
        if self.refuse {
            return Err("The map does not build.".into());
        }
        self.seen.push((play.clone(), base_counts.clone()));
        Ok(report())
    }
}

fn counts() -> BTreeMap<BlockKind, u32> {
    BTreeMap::from([(BlockKind::Crates, 2), (BlockKind::NetgameFlags, 3)])
}

fn session() -> Session {
    let palette = vec![PaletteItem {
        block: BlockKind::Crates,
        tag: TagKey {
            group: "bloc".into(),
            path: r"fixture\ramp".into(),
        },
    }];
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "court".into(),
            sha256: Sha256::of(b"an invented court"),
        },
        sources: vec![],
        title: "Ramps".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    };
    let limits = Limits {
        base_counts: counts(),
        world: Some(WorldBounds {
            min: [-30.; 3],
            max: [30.; 3],
        }),
        ..Default::default()
    };
    Session::new(base, limits, palette, 16).unwrap()
}

fn at(x: f32) -> Frame {
    Frame {
        position: [x, 0., 0.],
        rotation: [0.; 3],
    }
}

#[test]
fn a_generation_is_given_the_play_form_and_the_maps_counts_and_kept() {
    let mut s = session();
    let mut world = Counting::default();
    assert_eq!(s.nav_state(), NavState::NotBuilt);
    assert!(s.nav_report().is_none());
    s.place(&mut world, 1, 0, at(2.)).unwrap();

    let mut gen = Gen::default();
    let made = s.generate_nav(&mut gen).unwrap().clone();
    assert_eq!(made, report());
    assert_eq!(s.nav_state(), NavState::Current);
    assert_eq!(s.nav_report(), Some(&report()));
    let (play, base) = &gen.seen[0];
    assert_eq!(*play, forge_doc::play_form(s.document()));
    assert_eq!(*base, counts());
    assert_eq!(NavState::Current.name(), "current");
}

#[test]
fn an_edit_after_a_generation_makes_it_stale_and_generating_again_makes_it_current() {
    let mut s = session();
    let mut world = Counting::default();
    let mut gen = Gen::default();
    s.generate_nav(&mut gen).unwrap();
    s.place(&mut world, 1, 0, at(2.)).unwrap();
    assert_eq!(s.nav_state(), NavState::Stale);
    // The report stays until the next generation: the overlay shows it.
    assert!(s.nav_report().is_some());
    s.generate_nav(&mut gen).unwrap();
    assert_eq!(s.nav_state(), NavState::Current);
    assert_eq!(gen.seen.len(), 2);
    // Undo returns to the document the first generation saw, but only the
    // last generation is kept, so the nav is stale again.
    s.undo(&mut world, 2).unwrap();
    assert_eq!(s.nav_state(), NavState::Stale);
}

#[test]
fn a_failed_generation_keeps_the_last_one() {
    let mut s = session();
    let mut world = Counting::default();
    let mut gen = Gen::default();
    s.generate_nav(&mut gen).unwrap();
    s.place(&mut world, 1, 0, at(2.)).unwrap();
    gen.refuse = true;
    assert_eq!(
        s.generate_nav(&mut gen).unwrap_err(),
        "The map does not build."
    );
    assert_eq!(s.nav_state(), NavState::Stale);
    assert_eq!(s.nav_report(), Some(&report()));
}
