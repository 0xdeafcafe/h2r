//! Invented BSPs only; the real-map checks are in `tests/real_maps.rs`.

use h1map::Space;
use h1map::bsp::CollisionBsp;
use h1map::testing::collision_room;

use crate::collision::convert;
use crate::query::{first_hit, leaf_at};
use crate::sandbox::{H2Item, h2_equivalent, h2_name_matches};
use crate::*;

fn room() -> CollisionBsp {
    let base = 0x8190_0000;
    let (bytes, at) = collision_room(base);
    CollisionBsp::read(&Space::new(&bytes, base), &bytes[at..at + 0x60]).unwrap()
}

#[test]
fn the_room_converts_and_reads_back_as_halo_2_blocks() {
    let src = room();
    let h2 = convert(&src).unwrap();
    let bytes = h2.encode_blocks(0x8010_0000);
    let back = tags::collision::Bsp::decode_blocks(&bytes, 0x8010_0000, 0).unwrap();
    assert_eq!(back, h2);
    assert_eq!(back.validate(1), Vec::<String>::new());
    assert_eq!(back.nodes[5].front, tags::collision::Child::Leaf(0));
    assert_eq!(back.nodes[0].back, tags::collision::Child::None);
    assert_eq!(back.refs[0].node, 0x8000);
    assert_eq!(back.surfaces[0].breakable_surface, 0xff);
    assert_eq!(back.edges[0].right_surface, 0xffff);
    assert_eq!(back.surface_ring(0), vec![0, 1, 2, 3]);
}

#[test]
fn points_and_segments_agree_between_the_source_and_the_conversion() {
    let src = room();
    let h2 = convert(&src).unwrap();
    for p in [
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
        [0.5, 0.5, 1.5],
        [1.0, 1.0, 1.0],
    ] {
        assert_eq!(leaf_at(&src, p), leaf_at(&h2, p), "{p:?}");
        assert_eq!(leaf_at(&src, p), src.leaf_at(p).map(|l| l as usize));
    }
    // Straight down from the middle: the floor is half way.
    let down = first_hit(&src, [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]).unwrap();
    assert!((down - 0.5).abs() < 1e-9, "{down}");
    assert_eq!(
        first_hit(&h2, [0.5, 0.5, 0.5], [0.5, 0.5, -0.5]),
        Some(down)
    );
    // Inside the room: no hit. Starting in solid: hit at 0.
    assert_eq!(first_hit(&h2, [0.2, 0.2, 0.2], [0.8, 0.8, 0.8]), None);
    assert_eq!(first_hit(&h2, [2.0, 0.5, 0.5], [0.5, 0.5, 0.5]), Some(0.0));
}

#[test]
fn an_index_past_halo_2s_fields_is_refused() {
    let mut src = room();
    src.surfaces[0].plane = 0x8000;
    assert!(matches!(
        convert(&src),
        Err(Error::TooBig {
            what: "surface plane",
            ..
        })
    ));
    let mut src = room();
    src.leaves[0].ref_count = 300;
    assert!(matches!(convert(&src), Err(Error::TooBig { .. })));
    let mut src = room();
    src.nodes[0].front = h1map::bsp::Child::Node(0x80_0000);
    assert!(matches!(
        convert(&src),
        Err(Error::TooBig {
            what: "3D node",
            ..
        })
    ));
}

#[test]
fn flags_move_from_bit_31_to_bit_15() {
    let mut src = room();
    src.refs[0].plane = 0x8000_0004;
    src.surfaces[0].plane = 0x8000_0004;
    let h2 = convert(&src).unwrap();
    assert_eq!(h2.refs[0].plane, 0x8004);
    assert_eq!(h2.surfaces[0].plane, 0x8004);
}

#[test]
fn halo_1_items_map_to_their_nearest_halo_2_items() {
    let w = |p| h2_equivalent(p);
    assert_eq!(
        w("weapons\\assault rifle\\assault rifle"),
        Some(H2Item::Weapon("smg"))
    );
    assert_eq!(w("weapons\\pistol\\pistol"), Some(H2Item::Weapon("magnum")));
    assert_eq!(
        w("weapons\\plasma pistol\\plasma pistol"),
        Some(H2Item::Weapon("plasma_pistol"))
    );
    assert_eq!(
        w("powerups\\pistol ammo\\pistol ammo"),
        Some(H2Item::Dropped)
    );
    assert_eq!(w("powerups\\health pack"), Some(H2Item::Dropped));
    assert_eq!(
        w("vehicles\\ghost\\ghost_mp"),
        Some(H2Item::Vehicle("ghost"))
    );
    assert_eq!(w("something\\unknown"), None);
    assert!(h2_name_matches(
        "objects\\weapons\\rifle\\plasma_rifle\\plasma_rifle",
        "plasma_rifle"
    ));
    assert!(!h2_name_matches(
        "objects\\weapons\\rifle\\smg\\smg",
        "plasma_rifle"
    ));
}
