//! Pending pieces, ghosts and barriers as groups of the markers' pass, over
//! invented definitions.
use forge::piece::{self, PieceInfo, PieceState};
use forge::{Material, Materials, OpId, PieceDef, Shape};

use super::*;

fn info(def: PieceDef, state: PieceState) -> PieceInfo {
    let frame = Frame {
        position: [1., 2., 3.],
        rotation: [0.; 3],
    };
    PieceInfo {
        id: OpId(1),
        index: 0,
        name: def.name(),
        bounds: piece::bounds(&def, frame),
        def,
        state,
        frame,
    }
}

fn block() -> PieceDef {
    PieceDef::dev(Shape::Block, [100, 200, 300], 0)
}

fn barrier() -> PieceDef {
    PieceDef {
        materials: Materials::All(Material::Clip),
        ..block()
    }
}

#[test]
fn a_pending_piece_gives_its_shell_as_lines_and_a_skin_in_the_pending_tint() {
    let draws = draws(&[info(block(), PieceState::Pending)], false, None);
    assert_eq!(draws.len(), 1);
    let group = group(&draws[0]).expect("a block generates");
    // Six quads: 24 edges, 12 skin triangles.
    assert_eq!(group.edges.len(), 24);
    assert_eq!(group.tris.len(), 12);
    assert_eq!(group.colour, PENDING);
    assert!(group.tris.iter().all(|t| t.colours[0][3] == SKIN_ALPHA));
    // The block's lowest corner is the frame's origin.
    let min = group
        .edges
        .iter()
        .flatten()
        .fold([f32::INFINITY; 3], |m, p| std::array::from_fn(|i| m[i].min(p[i])));
    assert_eq!(min, [1., 2., 3.]);
}

#[test]
fn a_piece_with_an_object_draws_nothing_unless_it_is_a_barrier_and_the_overlay_is_on() {
    let live = |def| info(def, PieceState::Live);
    assert!(draws(&[live(block())], true, None).is_empty());
    assert!(draws(&[live(barrier())], false, None).is_empty());
    let on = draws(&[live(barrier())], true, None);
    assert_eq!(on.len(), 1);
    assert_eq!(on[0].look, Look::Barrier);
    assert_eq!(group(&on[0]).unwrap().colour, BARRIER);
}

#[test]
fn a_nodraw_role_draws_only_its_faces_with_the_overlay_on() {
    let mut each = [Material::Dev, Material::Dev, Material::Dev, Material::Dev];
    each[Role::Ceiling.index()] = Material::Nodraw;
    let def = PieceDef {
        materials: Materials::Each(each),
        ..block()
    };
    let on = draws(&[info(def, PieceState::Live)], true, None);
    assert_eq!(on[0].look, Look::Nodraw);
    let group = group(&on[0]).unwrap();
    assert_eq!(group.edges.len(), 4, "the one ceiling face");
    assert_eq!(group.colour, NODRAW);
}

#[test]
fn the_ghost_is_drawn_in_its_own_tint_whatever_the_overlay() {
    let ghost = (block(), Frame::default());
    let draws = draws(&[], false, Some(ghost));
    assert_eq!(draws.len(), 1);
    assert_eq!(group(&draws[0]).unwrap().colour, GHOST);
}

#[test]
fn publishing_makes_the_groups_the_source_draws() {
    let d = draws(&[info(block(), PieceState::Pending)], false, None);
    publish(&d);
    assert!(active());
    assert_eq!(published().len(), 1);
    clear();
    assert!(!active() && published().is_empty());
}
