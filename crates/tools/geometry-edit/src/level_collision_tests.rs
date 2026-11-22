//! A solid added to the level collision of an invented map
//! (`scenario_edit::fixture::fixture_with_collision` with a compiled box
//! room for its collision), saved through `scenario_edit::save` and read
//! back.
use super::*;
use collision_build::coll::BLOCK_NAMES;
use map::Map;
use scenario_edit::fixture::{self, META, META_FILE, SBSP, SBSP_COLLISION_AT, SBSP_SIZE};
use scenario_edit::{GeometryChanges, GeometryGrown, Scenario};
use std::io::Cursor;

/// Where the per-leaf block and the MOPP are, in the metadata past the
/// structure's payload.
const LEAVES_AT: usize = 0x1a00;
const MOPP_AT: usize = 0x1a40;
/// The structure's collision material count.
const MATERIALS: u32 = 2;

fn sbsp() -> TagId {
    fixture::id(4)
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A closed box room open inside `[lo, hi]`, faces in, material 1.
fn box_room(lo: [f32; 3], hi: [f32; 3]) -> Vec<Polygon> {
    let mut out = cube(lo, hi);
    for p in &mut out {
        p.points.reverse();
        p.material = 1;
    }
    out
}

/// A closed box, solid inside `[lo, hi]`, faces out, material 0.
fn cube(lo: [f32; 3], hi: [f32; 3]) -> Vec<Polygon> {
    let c = |i: usize| -> [f32; 3] {
        [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    let q = |is: [usize; 4]| Polygon::new(is.iter().map(|&i| c(i)).collect(), 0);
    vec![
        q([0, 4, 6, 2]),
        q([1, 3, 7, 5]),
        q([0, 1, 5, 4]),
        q([2, 6, 7, 3]),
        q([0, 2, 3, 1]),
        q([4, 5, 7, 6]),
    ]
}

fn room() -> Bsp {
    collision_build::build(&box_room([0.0; 3], [8.0, 8.0, 4.0])).unwrap().bsp
}

/// The per-leaf element of leaf `k`: cluster `k`, then six bytes that
/// differ per leaf.
fn leaf_element(k: usize) -> Vec<u8> {
    let mut e = (k as i16).to_le_bytes().to_vec();
    e.extend((0..6).map(|i| (k * 3 + i) as u8 ^ 0xc3));
    e
}

/// The invented map with [`room`] as its structure's collision, a
/// per-leaf block and a MOPP in a block of `mopp_capacity` bytes (at least
/// the program's own length), its metadata ending the file.
fn map_bytes(mopp_capacity: usize) -> Vec<u8> {
    let mut b = fixture::fixture_with_collision();
    let bsp = room();
    let root = META_FILE + SBSP;
    let blocks = META + (SBSP + SBSP_COLLISION_AT) as u32;
    let encoded = bsp.encode_blocks(blocks);
    assert!(SBSP_COLLISION_AT + encoded.len() <= SBSP_SIZE);
    let at = root + SBSP_COLLISION_AT;
    b[at..root + SBSP_SIZE].fill(0);
    b[at..at + encoded.len()].copy_from_slice(&encoded);
    put(&mut b, root + MATERIALS_FIELD, MATERIALS);
    put(&mut b, root + MATERIALS_FIELD + 4, META + 0x1900);
    let leaves: Vec<u8> = (0..bsp.leaves.len()).flat_map(leaf_element).collect();
    assert!(LEAVES_AT + leaves.len() <= MOPP_AT);
    b[META_FILE + LEAVES_AT..META_FILE + LEAVES_AT + leaves.len()].copy_from_slice(&leaves);
    put(&mut b, root + LEAVES_FIELD, bsp.leaves.len() as u32);
    put(&mut b, root + LEAVES_FIELD + 4, META + LEAVES_AT as u32);
    let mut mopp = mopp::build_structure(&bsp);
    assert!(mopp.len() <= mopp_capacity && MOPP_AT + mopp_capacity <= 0x2000);
    mopp.resize(mopp_capacity, 0);
    b[META_FILE + MOPP_AT..META_FILE + MOPP_AT + mopp.len()].copy_from_slice(&mopp);
    put(&mut b, root + MOPP_FIELD, mopp_capacity as u32);
    put(&mut b, root + MOPP_FIELD + 4, META + MOPP_AT as u32);
    fixture::metadata_last(b)
}

fn open(bytes: &[u8]) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes.to_vec())).unwrap()
}

/// The map with `w` saved through the scenario's save.
fn saved(src: &[u8], w: &LevelWrites) -> Vec<u8> {
    let mut map = open(src);
    let mut s = Scenario::read(&mut map).unwrap();
    let mut c = GeometryChanges::default();
    for p in &w.patches {
        match p.space {
            None => c.patches.push((p.address, p.bytes.clone())),
            Some(space) => c.bsp_patches.push(scenario_edit::BspPatch {
                space,
                address: p.address,
                bytes: p.bytes.clone(),
            }),
        }
    }
    c.grown = w
        .grown
        .iter()
        .map(|g| GeometryGrown {
            pair_space: g.pair_space,
            pair: g.pair,
            count: g.count,
            bytes: g.bytes.clone(),
            align: g.align,
        })
        .collect();
    s.set_geometry_changes(c).unwrap();
    let mut out = Cursor::new(Vec::new());
    scenario_edit::save::write_patched(&mut open(src), &s, &mut out).unwrap();
    out.into_inner()
}

fn solid() -> Vec<Polygon> {
    cube([2.0, 2.0, 1.0], [3.0, 3.0, 2.0])
}

#[test]
fn the_level_reads_as_the_map_holds_it() {
    let src = map_bytes(0x200);
    let level = read(&mut open(&src), sbsp()).unwrap();
    let bsp = room();
    assert_eq!(level.level.bsp, bsp);
    assert_eq!(level.level.materials, MATERIALS as usize);
    assert_eq!(level.leaves.count as usize, bsp.leaves.len());
    assert_eq!(level.level.leaves, (0..bsp.leaves.len()).flat_map(leaf_element).collect::<Vec<_>>());
    assert_eq!(level.mopp.count, 0x200);
    assert_eq!(&level.level.mopp[..mopp::build_structure(&bsp).len()], &mopp::build_structure(&bsp)[..]);
    // The fixture's structure is ordinary metadata.
    assert_eq!(level.space, None);
    assert!(level.blocks.iter().all(|b| b.space.is_none()));
    // Not a structure.
    assert!(matches!(read(&mut open(&src), fixture::id(1)), Err(Error::NoTarget(_))));
}

#[test]
fn an_added_solid_saves_as_grown_blocks_and_reads_back() {
    // A MOPP block with no room to spare: the new program is grown.
    let src = map_bytes(mopp::build_structure(&room()).len());
    let source = read(&mut open(&src), sbsp()).unwrap();
    let edit = source.add_solid(&solid(), &Options::default()).unwrap();
    let w = edit.writes();
    // Every block whose count changed is grown, none patched: the merge
    // only appends, and the old elements stay as they were.
    let new = &edit.added.level;
    let stage = edit.stage();
    for (i, b) in stage.blocks.iter().enumerate() {
        let grown = w.grown.iter().any(|g| g.name == BLOCK_NAMES[i]);
        assert_eq!(grown, b.count != source.blocks[i].count, "{}", b.name);
    }
    assert!(w.grown.iter().any(|g| g.name == "leaf_clusters"));
    assert!(w.grown.iter().any(|g| g.name == "mopp"));
    // Only the gated parent node's child changes in the old nodes; that
    // block grows, so nothing is patched.
    assert!(w.patches.is_empty(), "{:?}", w.patches);

    let out = saved(&src, &w);
    let mut m = open(&out);
    m.validate_checksum().unwrap();
    let back = read(&mut m, sbsp()).unwrap();
    assert_eq!(back.level, *new);
    // The grown blocks are in the metadata past its old end.
    let old_end = META + 0x2000;
    for (i, b) in back.blocks.iter().enumerate() {
        if b.count != source.blocks[i].count {
            assert!(b.address >= old_end, "{}", BLOCK_NAMES[i]);
        }
    }
    assert!(back.leaves.address >= old_end && back.mopp.address >= old_end);
    // The structure's payload differs only in the pairs it names.
    let before = open(&src).read_tag_bytes(sbsp()).unwrap();
    let after = m.read_tag_bytes(sbsp()).unwrap();
    let pairs: Vec<usize> = (0..8)
        .map(|i| SBSP_COLLISION_AT + 8 * i)
        .chain([LEAVES_FIELD, MOPP_FIELD])
        .collect();
    for (k, (x, y)) in before.iter().zip(&after).enumerate() {
        if x != y {
            assert!(pairs.iter().any(|&p| (p..p + 8).contains(&k)), "byte {k:#x} changed");
        }
    }
    // The live stage is the saved bytes.
    for (i, b) in stage.blocks.iter().enumerate() {
        assert_eq!((b.count, &b.bytes), (back.blocks[i].count, &back.block_bytes[i]), "{}", b.name);
    }
    assert_eq!(stage.leaves.bytes, back.level.leaves);
    assert_eq!(stage.mopp, back.level.mopp);
    // Added leaves carry their stand-in's element.
    for (i, &host) in edit.added.merged.stand_ins.iter().enumerate() {
        let at = (edit.added.merged.first_leaf + i) * LEAF_SIZE;
        assert_eq!(back.level.leaves[at..at + LEAF_SIZE], leaf_element(host)[..]);
    }
    // A second solid merges onto the saved level.
    let again = back
        .add_solid(&cube([5.0, 5.0, 1.0], [6.0, 6.0, 2.0]), &Options::default())
        .unwrap();
    let out2 = saved(&out, &again.writes());
    let mut m2 = open(&out2);
    m2.validate_checksum().unwrap();
    assert_eq!(read(&mut m2, sbsp()).unwrap().level, again.added.level);
}

#[test]
fn a_mopp_that_fits_is_written_over_the_old_program() {
    let src = map_bytes(0x5c0);
    let source = read(&mut open(&src), sbsp()).unwrap();
    let edit = source.add_solid(&solid(), &Options::default()).unwrap();
    let new_len = edit.added.level.mopp.len();
    assert!(new_len < 0x5c0);
    let w = edit.writes();
    assert!(!w.grown.iter().any(|g| g.name == "mopp"));
    // The count names the new length.
    let count_at = META + (SBSP + MOPP_FIELD) as u32;
    assert!(w.patches.iter().any(|p| p.address == count_at && p.bytes == (new_len as u32).to_le_bytes()));
    let out = saved(&src, &w);
    let mut m = open(&out);
    m.validate_checksum().unwrap();
    let back = read(&mut m, sbsp()).unwrap();
    assert_eq!(back.mopp.address, META + MOPP_AT as u32);
    assert_eq!(back.mopp.count as usize, new_len);
    assert_eq!(back.level, edit.added.level);
    // The old block's tail is zeroed.
    let at = m.file_offset(META + (MOPP_AT + new_len) as u32, (0x5c0 - new_len) as u32).unwrap();
    let tail = m.read_file_range(at, (0x5c0 - new_len) as u64).unwrap();
    assert!(tail.iter().all(|&x| x == 0));
}

#[test]
fn a_solid_that_cannot_be_added_is_refused_by_name() {
    let src = map_bytes(0x200);
    let source = read(&mut open(&src), sbsp()).unwrap();
    // Material 2 is past the structure's two.
    let mut polys = solid();
    polys[0].material = 2;
    let e = source.add_solid(&polys, &Options::default()).unwrap_err();
    match e {
        Error::Refused { target, reason } => {
            assert_eq!(target, "structure_collision:e1000004:0");
            assert!(reason.contains("material"), "{reason}");
        }
        e => panic!("{e}"),
    }
    // Outside the room: in the level's solid.
    let e = source
        .add_solid(&cube([20.0; 3], [21.0; 3]), &Options::default())
        .unwrap_err();
    assert!(e.to_string().contains("no open space"), "{e}");
}
