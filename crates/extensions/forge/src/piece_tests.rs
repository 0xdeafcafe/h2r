//! Block-out pieces in a session (docs/game/forge-blockout.md, slice BO7):
//! placing, changing and dropping, the states, pending, and the document
//! they leave. Invented definitions, an invented crates block and a fake
//! game that holds the definitions it is told of; no map, no tags.
use std::collections::BTreeMap;

use super::*;
use crate::piece;
use forge_doc::{
    BuilderVersions, Edit, Forge, ForgeDoc, Limits, MapRef, Placement, Sha256, WorldBounds,
};
use forge_shapes::{Material, Materials, PieceDef, Shape};

#[derive(Debug, PartialEq)]
enum Call {
    /// The handle, the `bloc`'s path and where.
    Place(u32, String, Frame),
    Delete(u32),
    Move(u32, Frame),
}

/// A game that holds the definitions in `loaded` and nothing else.
#[derive(Default)]
struct Game {
    loaded: Vec<String>,
    next: u32,
    objects: BTreeMap<u32, String>,
    calls: Vec<Call>,
}

impl Game {
    fn holding(defs: &[&PieceDef]) -> Self {
        Self {
            loaded: defs.iter().map(|d| d.name()).collect(),
            ..Default::default()
        }
    }

    fn make(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.next += 1;
        self.objects.insert(self.next, item.tag.path.clone());
        self.calls
            .push(Call::Place(self.next, item.tag.path.clone(), frame));
        Ok(self.next)
    }
}

impl World for Game {
    fn place(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.make(item, frame)
    }
    fn respawn(&mut self, item: &PaletteItem, frame: Frame) -> Result<u32, String> {
        self.make(item, frame)
    }
    fn move_object(&mut self, handle: u32, frame: Frame) -> Result<(), String> {
        self.calls.push(Call::Move(handle, frame));
        Ok(())
    }
    fn delete(&mut self, handle: u32) -> Result<(), String> {
        self.objects.remove(&handle).ok_or("stale handle")?;
        self.calls.push(Call::Delete(handle));
        Ok(())
    }
    fn piece_loaded(&self, name: &str) -> bool {
        self.loaded.iter().any(|n| n == name)
    }
}

fn session() -> Session {
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "court".into(),
            sha256: Sha256::of(b"an invented court"),
        },
        sources: vec![],
        title: "Blocks".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    };
    let limits = Limits {
        base_counts: BTreeMap::from([(BlockKind::Crates, 3)]),
        world: Some(WorldBounds {
            min: [-30.; 3],
            max: [30.; 3],
        }),
        ..Default::default()
    };
    Session::new(base, limits, vec![], 16).unwrap()
}

fn at(x: f32) -> Frame {
    Frame {
        position: [x, 1., 0.],
        rotation: [0.; 3],
    }
}

fn block() -> PieceDef {
    PieceDef::dev(Shape::Block, [100, 200, 300], 0)
}

fn ramp() -> PieceDef {
    PieceDef::dev(Shape::Ramp, [128, 128, 64], 0)
}

fn path(def: &PieceDef) -> String {
    def.tag_path()
}

#[test]
fn placing_a_loaded_definition_is_two_ops_one_undo_step_and_one_object_of_its_bloc() {
    let mut s = session();
    let def = block();
    let mut game = Game::holding(&[&def]);
    let id = s.place_piece(&mut game, 1, def.clone(), at(2.)).unwrap();

    let ops = &s.document().ops;
    assert_eq!(ops.len(), 2);
    let Edit::Placement(Placement::Add {
        block,
        like: None,
        fields,
    }) = &ops[0].edit
    else {
        panic!("the first op is the add: {:?}", ops[0]);
    };
    assert_eq!(*block, BlockKind::Crates);
    assert!(
        fields
            .iter()
            .any(|f| f.field == "palette index" && f.value == forge_doc::piece_value())
    );
    assert_eq!(ops[0].id, id);
    assert_eq!(
        ops[1].edit,
        Edit::Forge(Forge::Piece {
            target: Endpoint::Added(id),
            def: def.clone()
        })
    );
    assert_eq!(s.document().format, forge_doc::FORMAT_PIECES);

    // One call to the game: one object, of the definition's own bloc.
    assert_eq!(game.calls, [Call::Place(1, path(&def), at(2.))]);
    assert_eq!(s.objects().len(), 1);
    assert_eq!(s.objects()[0].item.tag, piece::bloc_key(&def));
    assert_eq!(s.objects()[0].item.tag.group, "bloc");

    let pieces = s.pieces();
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].state, PieceState::Live);
    assert_eq!(pieces[0].def, def);
    assert_eq!(pieces[0].name, def.name());
    // The crates block has three of the map's own: the piece is the fourth.
    assert_eq!(pieces[0].index, 3);
    assert_eq!(s.pending_pieces(), 0);
    assert_eq!(s.pending_rebuild(), 0);

    // One undo step takes both ops and the object.
    s.undo(&mut game, 2).unwrap();
    assert!(s.document().ops.is_empty());
    assert!(s.pieces().is_empty());
    assert!(s.objects().is_empty());
    assert_eq!(game.calls.last(), Some(&Call::Delete(1)));
    assert!(game.objects.is_empty());
    assert!(s.undo(&mut game, 3).is_err());
}

#[test]
fn a_definition_the_map_lacks_makes_no_engine_call_and_one_pending_piece() {
    let mut s = session();
    let mut game = Game::default();
    let def = ramp();
    let id = s.place_piece(&mut game, 1, def.clone(), at(0.)).unwrap();
    assert!(game.calls.is_empty());
    assert!(s.objects().is_empty());
    assert_eq!(s.document().ops.len(), 2);

    let pieces = s.pieces();
    assert_eq!(pieces.len(), 1);
    assert_eq!((pieces[0].id, pieces[0].state), (id, PieceState::Pending));
    assert_eq!(s.pending_pieces(), 1);
    // **Rebuild map** counts it.
    assert_eq!(s.pending_rebuild(), 1);

    // It is a thing the monitor can aim at, of the piece kind, with a
    // position and a rotation to edit and no palette object to swap.
    let thing = s.thing(Endpoint::Added(id)).unwrap();
    assert_eq!(thing.kind, ThingKind::Piece);
    assert_eq!(thing.frame, at(0.));
    let names: Vec<_> = props::of(&thing).map(|p| p.name).collect();
    assert_eq!(names, ["position", "rotation"]);
    assert_eq!(s.things().len(), 1);

    // Dropping, changing and removing it call the game for nothing.
    piece::drop(&mut s, &mut game, 2, id, at(4.)).unwrap();
    s.set_piece(&mut game, 3, id, block(), None).unwrap();
    s.delete(&mut game, 4, id).unwrap();
    assert!(game.calls.is_empty());
    assert!(s.pieces().is_empty());
    assert_eq!(s.pending_rebuild(), 0);
}

#[test]
fn after_adopt_copy_nothing_is_pending() {
    let mut s = session();
    let mut game = Game::default();
    let a = s.place_piece(&mut game, 1, ramp(), at(0.)).unwrap();
    let b = s.place_piece(&mut game, 2, block(), at(5.)).unwrap();
    assert_eq!(s.pending_pieces(), 2);

    // The map is rebuilt with both definitions and the game finds each
    // object at its row.
    let built = s.rebuild_form().unwrap();
    let baked = s.baked_objects(&built).unwrap();
    assert_eq!(
        baked,
        [(a, BlockKind::Crates, 3), (b, BlockKind::Crates, 4)]
    );
    let found = Reopened {
        handles: BTreeMap::from([(a, 40), (b, 41)]),
        room: None,
    };
    s.adopt_copy(&built, CopyKind::Rebuild, found).unwrap();
    assert_eq!(s.pending_pieces(), 0);
    assert_eq!(s.pending_rebuild(), 0);
    assert!(s.pieces().iter().all(|p| p.state == PieceState::Live));
    assert_eq!(
        s.objects().iter().map(|o| o.handle).collect::<Vec<_>>(),
        [40, 41]
    );

    // Bringing the world to the document asks it for nothing: the
    // objects stand.
    s.release(&mut game).unwrap();
    assert!(game.calls.is_empty());
}

#[test]
fn a_drop_deletes_the_object_and_makes_it_again_where_it_lands() {
    let mut s = session();
    let def = block();
    let mut game = Game::holding(&[&def]);
    let id = s.place_piece(&mut game, 1, def.clone(), at(0.)).unwrap();
    game.calls.clear();

    // Held, the piece does not move in the world: its body is fixed.
    s.hold(&mut game, 2, Endpoint::Added(id), at(3.)).unwrap();
    assert!(game.calls.is_empty());
    assert_eq!(s.document().ops.len(), 2);

    piece::drop(&mut s, &mut game, 3, id, at(3.)).unwrap();
    assert_eq!(
        game.calls,
        [Call::Delete(1), Call::Place(2, path(&def), at(3.))]
    );
    assert_eq!(s.document().ops.len(), 3);
    assert_eq!(s.pieces()[0].frame, at(3.));
    assert_eq!(s.pieces()[0].state, PieceState::Live);

    // The drop is one undo step: the object is made again where it was.
    game.calls.clear();
    s.undo(&mut game, 4).unwrap();
    assert_eq!(
        game.calls,
        [Call::Delete(2), Call::Place(3, path(&def), at(0.))]
    );
    assert_eq!(s.pieces()[0].frame, at(0.));

    // A drop where it stands writes nothing and calls nothing.
    game.calls.clear();
    piece::drop(&mut s, &mut game, 5, id, at(0.)).unwrap();
    assert!(game.calls.is_empty());
    assert_eq!(s.document().ops.len(), 2);
}

#[test]
fn undo_restores_the_definition_it_replaced() {
    let mut s = session();
    let (a, b) = (block(), PieceDef::dev(Shape::Block, [200, 200, 300], 0));
    let mut game = Game::holding(&[&a, &b]);
    let id = s.place_piece(&mut game, 1, a.clone(), at(0.)).unwrap();
    s.set_piece(&mut game, 2, id, b.clone(), None).unwrap();

    // One `Forge::Piece` on the same target, and a new object of the new
    // definition's bloc.
    assert_eq!(s.document().ops.len(), 3);
    assert_eq!(
        s.document().ops[2].edit,
        Edit::Forge(Forge::Piece {
            target: Endpoint::Added(id),
            def: b.clone()
        })
    );
    assert_eq!(s.pieces()[0].def, b);
    assert_eq!(
        game.calls[1..],
        [Call::Delete(1), Call::Place(2, path(&b), at(0.))]
    );

    s.undo(&mut game, 3).unwrap();
    assert_eq!(s.pieces()[0].def, a);
    assert_eq!(s.pieces()[0].name, a.name());
    assert_eq!(game.objects.values().collect::<Vec<_>>(), [&path(&a)]);

    s.redo(&mut game, 4).unwrap();
    assert_eq!(s.pieces()[0].def, b);
    assert_eq!(game.objects.values().collect::<Vec<_>>(), [&path(&b)]);
}

#[test]
fn a_change_that_moves_the_origin_is_one_step_and_one_that_changes_nothing_is_none() {
    let mut s = session();
    let (a, b) = (block(), ramp());
    let mut game = Game::holding(&[&a, &b]);
    let id = s.place_piece(&mut game, 1, a.clone(), at(0.)).unwrap();

    // The same definition at the same origin: nothing is written.
    s.set_piece(&mut game, 2, id, a.clone(), Some(at(0.))).unwrap();
    assert_eq!(s.document().ops.len(), 2);

    // A new definition and a new origin: the definition's op and the
    // placement's set, in one undo step.
    s.set_piece(&mut game, 3, id, b.clone(), Some(at(-2.))).unwrap();
    let ops = &s.document().ops;
    assert_eq!(ops.len(), 4);
    assert!(matches!(ops[2].edit, Edit::Forge(Forge::Piece { .. })));
    assert!(matches!(ops[3].edit, Edit::Placement(Placement::Set { .. })));
    assert_eq!(s.pieces()[0].def, b);
    assert_eq!(s.pieces()[0].frame, at(-2.));

    s.undo(&mut game, 4).unwrap();
    assert_eq!(s.document().ops.len(), 2);
    assert_eq!(s.pieces()[0].def, a);
    assert_eq!(s.pieces()[0].frame, at(0.));
}

#[test]
fn a_refused_definition_leaves_the_document_and_the_game_alone() {
    let mut s = session();
    let a = block();
    let mut game = Game::holding(&[&a]);
    // Too small on x.
    let tiny = PieceDef::dev(Shape::Block, [2, 64, 64], 0);
    assert!(s.place_piece(&mut game, 1, tiny.clone(), at(0.)).is_err());
    // A param the shape does not take.
    let sided = PieceDef::dev(Shape::Cylinder, [64, 64, 64], 7);
    assert!(s.place_piece(&mut game, 1, sided, at(0.)).is_err());
    // A material that is not a shader.
    let bad = PieceDef {
        materials: Materials::All(Material::Map(forge_shapes::TagKey {
            group: "bitm".into(),
            path: r"levels\rock".into(),
        })),
        ..a.clone()
    };
    assert!(s.place_piece(&mut game, 1, bad, at(0.)).is_err());
    assert!(s.document().ops.is_empty());
    assert!(game.calls.is_empty());

    let id = s.place_piece(&mut game, 2, a, at(0.)).unwrap();
    let before = s.document().ops.clone();
    assert!(s.set_piece(&mut game, 3, id, tiny, None).is_err());
    assert_eq!(s.document().ops, before);
    // Something that is not a piece is not changed as one.
    assert_eq!(
        s.set_piece(&mut game, 3, OpId(99), block(), None).unwrap_err(),
        "Aim at a Forge object."
    );
}

#[test]
fn removing_a_piece_removes_its_object_and_undo_brings_back_its_definition() {
    let mut s = session();
    let def = block();
    let mut game = Game::holding(&[&def]);
    let id = s.place_piece(&mut game, 1, def.clone(), at(1.)).unwrap();
    s.delete(&mut game, 2, id).unwrap();
    assert!(s.pieces().is_empty());
    assert!(s.objects().is_empty());
    assert!(game.objects.is_empty());
    assert!(matches!(
        s.document().ops.last().unwrap().edit,
        Edit::Placement(Placement::Remove { .. })
    ));

    s.undo(&mut game, 3).unwrap();
    assert_eq!(s.pieces().len(), 1);
    assert_eq!(s.pieces()[0].def, def);
    assert_eq!(s.pieces()[0].state, PieceState::Live);
    assert_eq!(game.objects.len(), 1);
}

#[test]
fn a_duplicate_takes_its_sources_definition() {
    let mut s = session();
    let def = PieceDef::dev(Shape::Stairs, [128, 64, 64], 4);
    let mut game = Game::holding(&[&def]);
    let id = s.place_piece(&mut game, 1, def.clone(), at(0.)).unwrap();
    let copy = s
        .duplicate(&mut game, 2, Endpoint::Added(id), at(6.))
        .unwrap();
    assert_ne!(copy, id);
    let pieces = s.pieces();
    assert_eq!(pieces.len(), 2);
    assert_eq!(pieces[0].def, pieces[1].def);
    assert_eq!(pieces[1].id, copy);
    assert_eq!(pieces[1].frame, at(6.));
    // Two placements of one definition share one name, so one set of tags.
    assert_eq!(pieces[0].name, pieces[1].name);
    assert_eq!(game.objects.len(), 2);
    // The copy is two ops in one undo step too.
    s.undo(&mut game, 3).unwrap();
    assert_eq!(s.pieces().len(), 1);
}

#[test]
fn a_shared_session_does_not_place_pieces() {
    let mut s = session().share(crate::net::Seat(1), "Guest");
    let mut game = Game::default();
    assert_eq!(
        s.place_piece(&mut game, 1, block(), at(0.)).unwrap_err(),
        PIECES_LOCAL_ONLY
    );
    assert!(s.take_outgoing().is_empty());
}

#[test]
fn a_pieces_bounds_follow_its_origin_and_its_turn() {
    let def = block();
    let near = |a: [f32; 3], b: [f32; 3]| {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} is not {b:?}");
        }
    };
    let flat = piece::bounds(&def, at(2.));
    assert_eq!(flat.min, [2., 1., 0.]);
    assert_eq!(flat.max, [3., 3., 3.]);

    // A quarter turn of yaw swings the box's x onto y and its y onto -x
    // about the origin, which stays a corner of the box.
    let turned = piece::bounds(
        &def,
        Frame {
            position: [2., 1., 0.],
            rotation: [std::f32::consts::FRAC_PI_2, 0., 0.],
        },
    );
    near(turned.min, [0., 1., 0.]);
    near(turned.max, [2., 2., 3.]);

    // Its sphere is the box's centre and its corner's distance.
    let mut s = session();
    let mut game = Game::holding(&[&def]);
    s.place_piece(&mut game, 1, def, at(0.)).unwrap();
    let extent = s.pieces()[0].extent();
    near(extent.offset, [0.5, 1., 1.5]);
    assert!((extent.radius - (0.25f32 + 1. + 2.25).sqrt()).abs() < 1e-6);
    let thing = s.thing(s.pieces()[0].target()).unwrap();
    let tool = crate::tool::Tool::default();
    assert_eq!(tool.thing_extent(&s, &thing), extent);
}

#[test]
fn a_pieces_definition_is_given_to_generate_nav_in_the_play_form() {
    #[derive(Default)]
    struct Gen {
        seen: Vec<ForgeDoc>,
    }
    impl NavBuilder for Gen {
        fn generate(
            &mut self,
            play: &ForgeDoc,
            _: &BTreeMap<BlockKind, u32>,
        ) -> Result<NavReport, String> {
            self.seen.push(play.clone());
            Ok(NavReport::default())
        }
    }
    let mut s = session();
    let def = ramp();
    // Pending or not, the generator builds the play form, whose build
    // installs the tags of every definition its pieces use: a piece is a
    // crate with a collision model, which the nav reads as a solid.
    let mut game = Game::default();
    s.place_piece(&mut game, 1, def.clone(), at(0.)).unwrap();
    let mut gen_ = Gen::default();
    s.generate_nav(&mut gen_).unwrap();
    let play = &gen_.seen[0];
    assert_eq!(*play, forge_doc::play_form(s.document()));
    assert_eq!(play.pieces(&s.limits().base_counts).unwrap().active.len(), 1);
    assert_eq!(play.format, forge_doc::FORMAT_PIECES);
    assert_eq!(s.nav_state(), NavState::Current);
    // Placing another makes the generation stale.
    s.place_piece(&mut game, 2, def, at(3.)).unwrap();
    assert_eq!(s.nav_state(), NavState::Stale);
}

#[test]
fn a_pieces_things_name_it_in_the_agents_spelling() {
    assert_eq!(ThingKind::Piece.name(), "piece");
    assert_eq!(PieceState::Live.name(), "live");
    assert_eq!(PieceState::Slot.name(), "slot");
    assert_eq!(PieceState::Pending.name(), "pending");
}
