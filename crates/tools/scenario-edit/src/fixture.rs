//! An invented multiplayer cache: every byte below is made up here. No
//! layout value, name or data comes from a game file. Used by this crate's
//! tests and by the editor's screenshot generator (docs/editor/editor-guide.md).
use map::{FourCC, TagId};

pub const INDEX: usize = 0x1000;
pub const META_FILE: usize = 0x1200;
pub const META: u32 = 0x9000_0000;
pub const SCNR: usize = 0x100; // payload offset inside metadata
pub const SCNR_SIZE: usize = 0x1000;
pub const FILE: usize = 0x4000;

pub fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
pub fn putf(b: &mut [u8], at: usize, v: f32) {
    put(b, at, v.to_bits());
}
pub fn id(slot: u32) -> TagId {
    TagId(0xe100_0000 | slot)
}
/// File offset of a scenario payload offset.
pub fn sfile(at: usize) -> usize {
    META_FILE + SCNR + at
}
pub fn block(b: &mut [u8], field: usize, count: u32, at: usize) {
    put(b, sfile(field), count);
    put(b, sfile(field) + 4, META + (SCNR + at) as u32);
}

pub const WEAPONS_AT: usize = 0x400;
pub const WEAPON_PALETTE_AT: usize = 0x4b0;
pub const STARTS_AT: usize = 0x500;
pub const FLAGS_AT: usize = 0x580;
pub const NETGAME_AT: usize = 0x600;
pub const SCENERY_AT: usize = 0x700;
pub const NAMES_AT: usize = 0x800;

/// The invented tags' names, in tag order, each ending in a NUL.
const NAMES: &[u8] =
    b"globals\0levels\\test\\fixture\0objects\\test\\blaster\0objects\\test\\kit\0";
const CLASSES: [[u8; 4]; 4] = [*b"matg", *b"scnr", *b"weap", *b"itmc"];
/// Each tag's (metadata offset, size).
const PAYLOADS: [(usize, usize); 4] =
    [(0, 0x10), (SCNR, SCNR_SIZE), (0x1100, 0x10), (0x1200, 0x10)];

/// Write the tag-name table: the NUL-separated names at 0x980, their
/// offsets at 0x900, one per tag.
fn tag_names(b: &mut [u8], names: &[u8]) {
    let count = names.iter().filter(|&&c| c == 0).count();
    b[0x980..0x980 + names.len()].copy_from_slice(names);
    put(b, 0x2c0, count as u32);
    put(b, 0x2c4, 0x980);
    put(b, 0x2c8, names.len() as u32);
    put(b, 0x2cc, 0x900);
    let mut at = 0;
    for (i, n) in names.split(|&c| c == 0).take(count).enumerate() {
        put(b, 0x900 + i * 4, at);
        at += n.len() as u32 + 1;
    }
}

/// Write the tag index: one group per class, then one tag per class
/// (tag `i` is [`id`]`(i)`, its payload at metadata offset
/// `payloads[i].0`). The scenario is tag 1 and the globals tag 0.
fn tag_index(b: &mut [u8], classes: &[[u8; 4]], payloads: &[(usize, usize)]) {
    let groups = INDEX + 32;
    let tags = groups + classes.len() * 12;
    for (i, c) in classes.iter().enumerate() {
        put(b, groups + i * 12, FourCC(*c).as_u32());
        put(b, groups + i * 12 + 4, u32::MAX);
        put(b, groups + i * 12 + 8, u32::MAX);
        let t = tags + i * 16;
        put(b, t, FourCC(*c).as_u32());
        put(b, t + 4, id(i as u32).0);
        put(b, t + 8, META + payloads[i].0 as u32);
        put(b, t + 12, payloads[i].1 as u32);
    }
    for (o, v) in [
        (0, 0x8000_0020),
        (4, classes.len() as u32),
        (8, 0x8000_0020 + 12 * classes.len() as u32),
        (12, id(1).0),
        (16, id(0).0),
        (24, classes.len() as u32),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(b, INDEX + o, v);
    }
}

/// A multiplayer cache with globals, a scenario, a weapon and a collection.
pub fn fixture() -> Vec<u8> {
    let mut b = vec![0u8; FILE];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    for (o, v) in [
        (4, 8),
        (8, FILE as u32),
        (0x10, INDEX as u32),
        (0x14, 0x200),
        (0x18, 0x2000),
        (0x1c, 0x2000),
    ] {
        put(&mut b, o, v);
    }
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    put(&mut b, 0x140, 1);
    b[0x198..0x19f].copy_from_slice(b"fixture");
    tag_names(&mut b, NAMES);
    tag_index(&mut b, &CLASSES, &PAYLOADS);
    // Unknown bytes everywhere in the block area, so preservation is tested.
    for (k, x) in b[sfile(0x400)..sfile(0x800)].iter_mut().enumerate() {
        *x = 0xa0 ^ (k as u8);
    }
    // Weapons: two placements, one palette entry naming the weapon tag.
    block(&mut b, 0x90, 2, WEAPONS_AT);
    block(&mut b, 0x98, 1, WEAPON_PALETTE_AT);
    for i in 0..2 {
        let e = sfile(WEAPONS_AT + i * 0x54);
        b[e..e + 2].copy_from_slice(&0i16.to_le_bytes());
        for k in 0..3 {
            putf(&mut b, e + 8 + 4 * k, (i * 10 + k) as f32);
            putf(&mut b, e + 0x14 + 4 * k, 0.0);
        }
        putf(&mut b, e + 0x20, 1.0);
    }
    put(&mut b, sfile(WEAPON_PALETTE_AT), FourCC(*b"weap").as_u32());
    put(&mut b, sfile(WEAPON_PALETTE_AT) + 4, id(2).0);
    // Two starting locations, one flag, one netgame item, one scenery.
    block(&mut b, 0x100, 2, STARTS_AT);
    for i in 0..2 {
        let e = sfile(STARTS_AT + i * 0x34);
        for k in 0..3 {
            putf(&mut b, e + 4 * k, -(i as f32) - k as f32);
        }
        putf(&mut b, e + 0xc, 0.5);
        b[e + 0x10..e + 0x12].copy_from_slice(&1i16.to_le_bytes());
    }
    block(&mut b, 0x118, 1, FLAGS_AT);
    let e = sfile(FLAGS_AT);
    for k in 0..4 {
        putf(&mut b, e + 4 * k, 3.0);
    }
    b[e + 0x10..e + 0x14].copy_from_slice(&[2, 0, 1, 0]);
    block(&mut b, 0x120, 1, NETGAME_AT);
    let e = sfile(NETGAME_AT);
    b[e + 0xe..e + 0x10].copy_from_slice(&30i16.to_le_bytes());
    b[e + 0x14] = 0;
    for k in 0..6 {
        putf(&mut b, e + 0x40 + 4 * k, 1.0);
    }
    put(&mut b, e + 0x58, FourCC(*b"itmc").as_u32());
    put(&mut b, e + 0x5c, id(3).0);
    block(&mut b, 0x50, 1, SCENERY_AT);
    let e = sfile(SCENERY_AT);
    b[e..e + 2].copy_from_slice(&(-1i16).to_le_bytes());
    for k in 0..3 {
        putf(&mut b, e + 8 + 4 * k, 5.0);
        putf(&mut b, e + 0x14 + 4 * k, 0.0);
    }
    putf(&mut b, e + 0x20, 0.0);
    // Object names: (type, placement) = weapon 1, scenery 0, weapon 0.
    block(&mut b, 0x48, 3, NAMES_AT);
    for (i, (ty, p)) in [(2i16, 1i16), (6, 0), (2, 0)].into_iter().enumerate() {
        let e = sfile(NAMES_AT + i * 0x24);
        b[e..e + 4].copy_from_slice(b"nm\0\0");
        b[e + 0x20..e + 0x22].copy_from_slice(&ty.to_le_bytes());
        b[e + 0x22..e + 0x24].copy_from_slice(&p.to_le_bytes());
    }
    b
}

pub const VOLUMES_AT: usize = 0x900;
pub const KILLS_AT: usize = 0x990;
pub const SWITCHES_AT: usize = 0x9a0;

/// The trigger volumes [`fixture_with_volumes`] adds: (origin corner,
/// forward, full size). Volume 0 is axis-aligned; volume 1 is turned a
/// quarter (forward +y) and is the one the kill trigger names; the BSP
/// switch names volume 0, from BSP 0 to 1. Both are far from the
/// placements, so a ray at one meets no glyph.
pub const VOLUMES: [([f32; 3], [f32; 3], [f32; 3]); 2] = [
    ([100.0, 100.0, 0.0], [1.0, 0.0, 0.0], [4.0, 4.0, 4.0]),
    ([-40.0, -40.0, -10.0], [0.0, 1.0, 0.0], [6.0, 2.0, 3.0]),
];

/// [`fixture`] with two trigger volumes, a kill trigger and a BSP switch.
pub fn fixture_with_volumes() -> Vec<u8> {
    let mut b = fixture();
    block(&mut b, 0x108, VOLUMES.len() as u32, VOLUMES_AT);
    for (i, (origin, forward, size)) in VOLUMES.into_iter().enumerate() {
        let e = sfile(VOLUMES_AT + i * 0x44);
        // No name, no parent object.
        b[e + 4..e + 6].copy_from_slice(&(-1i16).to_le_bytes());
        for (at, v) in [
            (0xc, forward),
            (0x18, [0.0, 0.0, 1.0]),
            (0x24, origin),
            (0x30, size),
        ] {
            for (k, x) in v.into_iter().enumerate() {
                putf(&mut b, e + at + 4 * k, x);
            }
        }
    }
    block(&mut b, 0x230, 1, KILLS_AT);
    let e = sfile(KILLS_AT);
    b[e..e + 2].copy_from_slice(&1i16.to_le_bytes());
    block(&mut b, 0x130, 1, SWITCHES_AT);
    let e = sfile(SWITCHES_AT);
    for (k, v) in [0i16, 0, 1].into_iter().enumerate() {
        b[e + 2 * k..e + 2 * k + 2].copy_from_slice(&v.to_le_bytes());
    }
    b
}

/// The fixture with its metadata region ending the file, as in retail maps,
/// so a block can grow.
pub fn fixture_at_end() -> Vec<u8> {
    at_end(fixture())
}

/// [`fixture_with_volumes`] with its metadata ending the file, so the
/// volume blocks can grow.
pub fn fixture_with_volumes_at_end() -> Vec<u8> {
    at_end(fixture_with_volumes())
}

/// `b` with a copy of its tag index and metadata placed after everything
/// else and the header pointed at it, so the metadata ends the file while
/// raw data past the old metadata stays where its pointers say (the old
/// copy is left as dead bytes). A grown section's block can then be
/// inserted before the index (`map::MapEditor::append_raw`).
pub fn metadata_last(mut b: Vec<u8>) -> Vec<u8> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let index = word(&b, 0x10) as usize;
    let len = (word(&b, 0x14) + word(&b, 0x18)) as usize;
    let moved = b.len();
    let copy = b[index..index + len].to_vec();
    b.extend(copy);
    put(&mut b, 0x10, moved as u32);
    let len = b.len() as u32;
    put(&mut b, 8, len);
    b
}

/// [`fixture_with_textured_bsp`] with its metadata ending the file
/// ([`metadata_last`]).
pub fn fixture_with_textured_bsp_last() -> Vec<u8> {
    metadata_last(fixture_with_textured_bsp())
}

fn at_end(mut b: Vec<u8>) -> Vec<u8> {
    let end = META_FILE + 0x2000;
    b.truncate(end);
    put(&mut b, 8, end as u32);
    b
}

/// Where [`fixture_with_collision`]'s structure BSP payload is, in the
/// metadata, and its size.
pub const SBSP: usize = 0x1300;
pub const SBSP_SIZE: usize = 0x600;
/// The collision BSP's blocks inside that payload.
pub const SBSP_COLLISION_AT: usize = 0x200;
/// The scenario's structure BSP block (`scnr+0x210`, one 0x44 entry).
pub const BSPS_AT: usize = 0xa00;
/// The corner of [`collision_room`] in [`fixture_with_collision`], far
/// from the placements and volumes.
pub const ROOM_AT: [f32; 3] = [200.0, 200.0, 0.0];

/// An invented collision BSP of quads, each (four corners, plane). Each
/// quad's four edges run round it on their own (left surface the quad,
/// right none); no node, leaf or 2D tree, since only the surfaces are read.
/// Surface 2 has the breakable flag (bit 3).
pub fn collision_quads(quads: &[([[f32; 3]; 4], [f32; 4])]) -> tags::collision::Bsp {
    use tags::collision::{Bsp, Edge, Plane, Surface, Vertex};
    let mut bsp = Bsp::default();
    for (q, (corners, plane)) in quads.iter().enumerate() {
        let v0 = bsp.vertices.len() as u16;
        let e0 = bsp.edges.len() as u16;
        for c in corners {
            bsp.vertices.push(Vertex {
                point: *c,
                edge: u32::from(e0),
            });
        }
        for k in 0..4u16 {
            bsp.edges.push(Edge {
                start_vertex: v0 + k,
                end_vertex: v0 + (k + 1) % 4,
                forward_edge: e0 + (k + 1) % 4,
                reverse_edge: e0 + (k + 3) % 4,
                left_surface: q as u16,
                right_surface: 0xffff,
            });
        }
        bsp.planes.push(Plane(*plane));
        bsp.surfaces.push(Surface {
            plane: q as u16,
            first_edge: e0,
            flags: if q == 2 { 0x08 } else { 0 },
            breakable_surface: 0xff,
            material: q as u16,
        });
    }
    bsp
}

/// The invented room with its corner at `at`: surface 0 a 10 by 10
/// floor, surface 1 a wall 4 high on the room's low-x side facing in,
/// surface 2 the same wall on the high-x side.
pub fn collision_room(at: [f32; 3]) -> tags::collision::Bsp {
    let p = |x: f32, y: f32, z: f32| [at[0] + x, at[1] + y, at[2] + z];
    collision_quads(&[
        (
            [
                p(0.0, 0.0, 0.0),
                p(10.0, 0.0, 0.0),
                p(10.0, 10.0, 0.0),
                p(0.0, 10.0, 0.0),
            ],
            [0.0, 0.0, 1.0, at[2]],
        ),
        (
            [
                p(0.0, 0.0, 0.0),
                p(0.0, 10.0, 0.0),
                p(0.0, 10.0, 4.0),
                p(0.0, 0.0, 4.0),
            ],
            [1.0, 0.0, 0.0, at[0]],
        ),
        (
            [
                p(10.0, 0.0, 0.0),
                p(10.0, 0.0, 4.0),
                p(10.0, 10.0, 4.0),
                p(10.0, 10.0, 0.0),
            ],
            [-1.0, 0.0, 0.0, -(at[0] + 10.0)],
        ),
    ])
}

/// [`fixture`] with a fifth tag, a structure BSP the scenario names
/// (`scnr+0x210`), whose collision BSP is [`collision_room`] at
/// [`ROOM_AT`]. It has no render geometry and no lightmap, so nothing
/// drawn covers any of its surfaces.
pub fn fixture_with_collision() -> Vec<u8> {
    let mut b = fixture();
    let mut names = NAMES.to_vec();
    names.extend(b"levels\\test\\room\0");
    tag_names(&mut b, &names);
    let mut classes = CLASSES.to_vec();
    classes.push(*b"sbsp");
    let mut payloads = PAYLOADS.to_vec();
    payloads.push((SBSP, SBSP_SIZE));
    tag_index(&mut b, &classes, &payloads);
    // The structure BSP: an empty root but for its collision block.
    let root = META_FILE + SBSP;
    let blocks = META + (SBSP + SBSP_COLLISION_AT) as u32;
    let field = tags::pathfinding::SBSP_COLLISION_FIELD;
    put(&mut b, root + field, 1);
    put(&mut b, root + field + 4, blocks);
    let encoded = collision_room(ROOM_AT).encode_blocks(blocks);
    assert!(SBSP_COLLISION_AT + encoded.len() <= SBSP_SIZE);
    let at = root + SBSP_COLLISION_AT;
    b[at..at + encoded.len()].copy_from_slice(&encoded);
    // The scenario names it: tag reference at +0x10, no lightmap.
    block(&mut b, 0x210, 1, BSPS_AT);
    let e = sfile(BSPS_AT);
    put(&mut b, e + 0x10, FourCC(*b"sbsp").as_u32());
    put(&mut b, e + 0x14, id(4).0);
    put(&mut b, e + 0x18, u32::MAX);
    put(&mut b, e + 0x1c, u32::MAX);
    b
}

/// Where [`fixture_with_pathfinding`]'s pathfinding element and its
/// blocks are in the structure BSP payload, after the collision.
pub const PATHFINDING_AT: usize = 0x400;
/// The two invented sectors' flags: different, so they tint differently.
pub const SECTOR_FLAGS: [u16; 2] = [0x0019, 0x0008];

/// [`fixture_with_collision`] with invented pathfinding data
/// (`sbsp+0xc4`, docs/formats/pathfinding.md) over [`collision_room`]'s
/// floor: sector 0 its low-x half, sector 1 its high-x half, both
/// counter-clockwise from above, sharing link 1. Links 0 to 6 ring the two
/// sectors; link 7 is an orphan (no sector either side). The floor's ref
/// names a 2D BSP node splitting the room at x 5; the walls' name nothing.
pub fn fixture_with_pathfinding() -> Vec<u8> {
    let mut b = fixture_with_collision();
    let root = META_FILE + SBSP;
    let addr = |at: usize| META + (SBSP + at) as u32;
    let element = PATHFINDING_AT;
    let size = tags::pathfinding::PathfindingData::SIZE;
    // Every block after the element, in order: (field, count, bytes).
    let [x, y, z] = ROOM_AT;
    let vertices: [[f32; 3]; 6] = [
        [x, y, z],
        [x + 5.0, y, z],
        [x + 5.0, y + 10.0, z],
        [x, y + 10.0, z],
        [x + 10.0, y, z],
        [x + 10.0, y + 10.0, z],
    ];
    const NONE: u16 = 0xffff;
    // vertex1, vertex2, flags, hint, forward, reverse, left, right.
    let links: [[u16; 8]; 8] = [
        [0, 1, 0, NONE, 1, NONE, 0, NONE],
        [1, 2, 0x40, NONE, 2, 4, 0, 1],
        [2, 3, 0, NONE, 3, NONE, 0, NONE],
        [3, 0, 0, NONE, 0, NONE, 0, NONE],
        [1, 4, 0, NONE, 5, NONE, 1, NONE],
        [4, 5, 0, NONE, 6, NONE, 1, NONE],
        [5, 2, 0, NONE, 1, NONE, 1, NONE],
        [0, 4, 0, NONE, NONE, NONE, NONE, NONE],
    ];
    let mut sectors = Vec::new();
    for (flags, first) in [(SECTOR_FLAGS[0], 0u32), (SECTOR_FLAGS[1], 4)] {
        sectors.extend(flags.to_le_bytes());
        sectors.extend(NONE.to_le_bytes());
        sectors.extend(first.to_le_bytes());
    }
    let links: Vec<u8> = links
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let refs: Vec<u8> = [0x8000_0000u32, u32::MAX, u32::MAX]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    // The line x = 205: positive (x >= 205) sector 1, negative sector 0.
    let mut nodes = Vec::new();
    for v in [1.0f32, 0.0, x + 5.0] {
        nodes.extend(v.to_le_bytes());
    }
    nodes.extend(1u32.to_le_bytes());
    nodes.extend(0u32.to_le_bytes());
    let vertices: Vec<u8> = vertices
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let blocks: [(usize, u32, Vec<u8>); 5] = [
        (0x00, 2, sectors),
        (0x08, 8, links),
        (0x10, 3, refs),
        (0x18, 1, nodes),
        (0x28, 6, vertices),
    ];
    let mut at = element + size;
    for (field, count, bytes) in blocks {
        put(&mut b, root + element + field, count);
        put(&mut b, root + element + field + 4, addr(at));
        b[root + at..root + at + bytes.len()].copy_from_slice(&bytes);
        at += bytes.len();
    }
    assert!(at <= SBSP_SIZE, "the pathfinding blocks fit the payload");
    let field = tags::pathfinding::SBSP_PATHFINDING_FIELD;
    put(&mut b, root + field, 1);
    put(&mut b, root + field + 4, addr(element));
    b
}

/// Where [`fixture_with_instance_collision`]'s instanced geometry
/// definition and its placement are in the structure BSP payload.
pub const INSTANCE_DEF_AT: usize = 0x400;
pub const INSTANCE_AT: usize = 0x580;
/// Where that placement puts the definition's unit quad (its corner).
pub const INSTANCE_POSITION: [f32; 3] = [202.0, 202.0, 3.0];

/// The unit quad in the `z = 0` plane, the instance definition's
/// collision.
pub fn collision_unit_quad() -> tags::collision::Bsp {
    collision_quads(&[(
        [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        [0.0, 0.0, 1.0, 0.0],
    )])
}

/// [`fixture_with_collision`] with one instanced geometry definition
/// (`sbsp+0x138`) whose collision BSP is [`collision_unit_quad`], and one
/// placement of it (`sbsp+0x140`) at [`INSTANCE_POSITION`], unturned.
pub fn fixture_with_instance_collision() -> Vec<u8> {
    use tags::structure::{InstancedGeometryDefinition as Def, InstancedGeometryInstance as Inst};
    let mut b = fixture_with_collision();
    let root = META_FILE + SBSP;
    let addr = |at: usize| META + (SBSP + at) as u32;
    let data_at = INSTANCE_DEF_AT + Def::SIZE;
    // The fields go in the definition, the blocks' data after it.
    let encoded = collision_unit_quad().encode_blocks(addr(data_at) - 0x40);
    assert!(data_at + encoded.len() - 0x40 <= INSTANCE_AT);
    assert!(INSTANCE_AT + Inst::SIZE <= SBSP_SIZE);
    let fields = root + INSTANCE_DEF_AT + Def::COLLISION;
    b[fields..fields + 0x40].copy_from_slice(&encoded[..0x40]);
    let data = root + data_at;
    b[data..data + encoded.len() - 0x40].copy_from_slice(&encoded[0x40..]);
    put(&mut b, root + tags::structure::SBSP_INSTANCE_DEFINITIONS, 1);
    put(
        &mut b,
        root + tags::structure::SBSP_INSTANCE_DEFINITIONS + 4,
        addr(INSTANCE_DEF_AT),
    );
    put(&mut b, root + 0x140, 1);
    put(&mut b, root + 0x144, addr(INSTANCE_AT));
    let p = root + INSTANCE_AT;
    // Scale 1, forward x, left y, up z, the position; definition 0.
    for (o, v) in [(0, 1.0), (4, 1.0), (0x14, 1.0), (0x24, 1.0)] {
        putf(&mut b, p + o, v);
    }
    for (k, v) in INSTANCE_POSITION.into_iter().enumerate() {
        putf(&mut b, p + 0x28 + 4 * k, v);
    }
    b
}

/// [`fixture_with_object_collision`]'s tags: a scenery, its model, the
/// model's `coll` and its `phmo`, at these metadata offsets.
pub const OBJ_SCEN: usize = 0x1300;
pub const OBJ_HLMT: usize = 0x1400;
pub const OBJ_COLL: usize = 0x1600;
pub const OBJ_COLL_SIZE: usize = 0x500;
pub const OBJ_PHMO: usize = 0x1b00;
pub const OBJ_PHMO_SIZE: usize = 0x300;
/// The scenario's scenery palette (`scnr+0x58`, one 0x28 entry).
pub const SCENERY_PALETTE_AT: usize = 0xa80;
/// The model's two node names (invented string IDs: length in the top
/// byte, index below): a root, and a child one unit above it. Both are in
/// the string-ID table [`fixture_with_object_collision`] writes.
pub const NODE_NAMES: [u32; 2] = [0x0400_0001, 0x0500_0002];
/// The invented string-ID table, by index.
const STRING_IDS: [&str; 3] = ["", "root", "child"];

/// Write the string-ID table: the offsets at 0xc00, the NUL-separated
/// strings at 0xc40.
fn string_ids(b: &mut [u8], strings: &[&str]) {
    let (index, data) = (0xc00, 0xc40);
    let mut at = 0;
    for (i, s) in strings.iter().enumerate() {
        put(b, index + i * 4, at as u32);
        b[data + at..data + at + s.len()].copy_from_slice(s.as_bytes());
        b[data + at + s.len()] = 0;
        at += s.len() + 1;
    }
    assert!(data + at <= INDEX);
    put(b, 0x164, strings.len() as u32);
    put(b, 0x168, at as u32);
    put(b, 0x16c, index as u32);
    put(b, 0x170, data as u32);
}
/// The `phmo` sphere: its radius and its centre in the root node's space.
pub const SPHERE_RADIUS: f32 = 0.5;
pub const SPHERE_CENTER: [f32; 3] = [0.0, 0.0, 0.5];

/// A unit cube round the origin as six quads, facing out.
pub fn collision_cube() -> tags::collision::Bsp {
    let c = |x: f32, y: f32, z: f32| [x - 0.5, y - 0.5, z - 0.5];
    collision_quads(&[
        (
            [c(0., 0., 0.), c(0., 1., 0.), c(1., 1., 0.), c(1., 0., 0.)],
            [0.0, 0.0, -1.0, 0.5],
        ),
        (
            [c(0., 0., 1.), c(1., 0., 1.), c(1., 1., 1.), c(0., 1., 1.)],
            [0.0, 0.0, 1.0, 0.5],
        ),
        (
            [c(0., 0., 0.), c(1., 0., 0.), c(1., 0., 1.), c(0., 0., 1.)],
            [0.0, -1.0, 0.0, 0.5],
        ),
        (
            [c(0., 1., 0.), c(0., 1., 1.), c(1., 1., 1.), c(1., 1., 0.)],
            [0.0, 1.0, 0.0, 0.5],
        ),
        (
            [c(0., 0., 0.), c(0., 0., 1.), c(0., 1., 1.), c(0., 1., 0.)],
            [-1.0, 0.0, 0.0, 0.5],
        ),
        (
            [c(1., 0., 0.), c(1., 1., 0.), c(1., 1., 1.), c(1., 0., 1.)],
            [1.0, 0.0, 0.0, 0.5],
        ),
    ])
}

/// [`fixture`] whose scenery (at `[5, 5, 5]`, unturned) places an invented
/// `scen` tag. Its model (`hlmt`) has two nodes, the second one unit above
/// the first; its `coll` is [`collision_cube`] on the second node (the BSP's
/// leading word, through the `coll` node names); its `phmo` is one sphere
/// ([`SPHERE_RADIUS`] at [`SPHERE_CENTER`]) on a rigid body of the first
/// node. Tags 4 to 7.
pub fn fixture_with_object_collision() -> Vec<u8> {
    let mut b = fixture();
    let mut names = NAMES.to_vec();
    names.extend(
        b"objects\\test\\crate\0objects\\test\\crate_model\0objects\\test\\crate_coll\0objects\\test\\crate_phmo\0",
    );
    tag_names(&mut b, &names);
    string_ids(&mut b, &STRING_IDS);
    let mut classes = CLASSES.to_vec();
    classes.extend([*b"scen", *b"hlmt", *b"coll", *b"phmo"]);
    let mut payloads = PAYLOADS.to_vec();
    payloads.extend([
        (OBJ_SCEN, 0x100),
        (OBJ_HLMT, 0x200),
        (OBJ_COLL, OBJ_COLL_SIZE),
        (OBJ_PHMO, OBJ_PHMO_SIZE),
    ]);
    tag_index(&mut b, &classes, &payloads);
    let file = |at: usize| META_FILE + at;
    let addr = |at: usize| META + at as u32;
    let null = |b: &mut [u8], at: usize| {
        put(b, at, u32::MAX);
        put(b, at + 4, u32::MAX);
    };
    // The scenery tag: object type 6, its model, null references.
    let s = file(OBJ_SCEN);
    b[s..s + 2].copy_from_slice(&6u16.to_le_bytes());
    putf(&mut b, s + 4, 1.0);
    put(&mut b, s + 0x34, FourCC(*b"hlmt").as_u32());
    put(&mut b, s + 0x38, id(5).0);
    for o in [0x3c, 0x44, 0x4c, 0x54] {
        null(&mut b, s + o);
    }
    // The model: collision and physics references, two nodes.
    let h = file(OBJ_HLMT);
    for o in [0, 0x10, 0x18, 0x8c, 0x94, 0xf0] {
        null(&mut b, h + o);
    }
    put(&mut b, h + 8, FourCC(*b"coll").as_u32());
    put(&mut b, h + 0xc, id(6).0);
    put(&mut b, h + 0x20, FourCC(*b"phmo").as_u32());
    put(&mut b, h + 0x24, id(7).0);
    let nodes_at = OBJ_HLMT + 0x100;
    put(&mut b, h + 0x78, 2);
    put(&mut b, h + 0x7c, addr(nodes_at));
    for (i, (parent, z)) in [(-1i16, 0.0f32), (0, 1.0)].into_iter().enumerate() {
        let n = file(nodes_at + i * 0x5c);
        put(&mut b, n, NODE_NAMES[i]);
        b[n + 4..n + 6].copy_from_slice(&parent.to_le_bytes());
        b[n + 6..n + 10].copy_from_slice(&[0xff; 4]);
        putf(&mut b, n + 0x14, z);
        putf(&mut b, n + 0x24, 1.0);
        putf(&mut b, n + 0x28, 1.0);
    }
    // The coll: one region, one permutation, the cube on coll node 1.
    let c = file(OBJ_COLL);
    let (region, perm, bsp) = (OBJ_COLL + 0x40, OBJ_COLL + 0x50, OBJ_COLL + 0x70);
    put(&mut b, c + 0x1c, 1);
    put(&mut b, c + 0x20, addr(region));
    put(&mut b, file(region) + 4, 1);
    put(&mut b, file(region) + 8, addr(perm));
    put(&mut b, file(perm) + 4, 1);
    put(&mut b, file(perm) + 8, addr(bsp));
    put(&mut b, file(bsp), 1);
    let encoded = collision_cube().encode_blocks(addr(bsp + 4));
    let nodes = OBJ_COLL + 0x400;
    assert!(bsp + 4 + encoded.len() <= nodes);
    b[file(bsp + 4)..file(bsp + 4) + encoded.len()].copy_from_slice(&encoded);
    put(&mut b, c + 0x2c, 2);
    put(&mut b, c + 0x30, addr(nodes));
    for (i, name) in NODE_NAMES.into_iter().enumerate() {
        let n = file(nodes + i * 12);
        put(&mut b, n, name);
        b[n + 4..n + 12].copy_from_slice(&[0xff; 8]);
    }
    // The phmo: one sphere, one rigid body on node 0 holding it.
    let p = file(OBJ_PHMO);
    let (sphere, body) = (OBJ_PHMO + 0x110, OBJ_PHMO + 0x190);
    put(&mut b, p + 0x48, 1);
    put(&mut b, p + 0x4c, addr(sphere));
    putf(&mut b, file(sphere) + 0x2c, SPHERE_RADIUS);
    for k in 0..3 {
        putf(&mut b, file(sphere) + 0x40 + k * 0x14, 1.0);
        putf(&mut b, file(sphere) + 0x70 + 4 * k, SPHERE_CENTER[k]);
    }
    put(&mut b, p + 0x38, 1);
    put(&mut b, p + 0x3c, addr(body));
    // Node 0, shape {sphere, 0}.
    put(&mut b, file(body) + 0x38, 0);
    // The scenery palette names the tag; the scenery places entry 0.
    block(&mut b, 0x58, 1, SCENERY_PALETTE_AT);
    put(&mut b, sfile(SCENERY_PALETTE_AT), FourCC(*b"scen").as_u32());
    put(&mut b, sfile(SCENERY_PALETTE_AT) + 4, id(4).0);
    b[sfile(SCENERY_AT)..sfile(SCENERY_AT) + 2].copy_from_slice(&0i16.to_le_bytes());
    b
}

/// Where [`fixture_with_textured_bsp`]'s tags are in the metadata, and
/// their sizes: the structure BSP, its lightmap, the floor's shader, its
/// base map and the lightmap page.
pub const TEX_SBSP: usize = 0x1300;
pub const TEX_SBSP_SIZE: usize = 0x400;
pub const TEX_LTMP: usize = 0x1700;
pub const TEX_LTMP_SIZE: usize = 0x600;
pub const TEX_SHAD: usize = 0x1d00;
pub const TEX_BASE: usize = 0x1e00;
pub const TEX_PAGE: usize = 0x1f00;
/// Where the section's raw block and the two bitmaps' pixels are in the
/// file (past the metadata), and each bitmap LOD's size.
pub const TEX_BLOCK_FILE: usize = 0x3200;
pub const TEX_BASE_FILE: usize = 0x3600;
pub const TEX_PAGE_FILE: usize = 0x3700;
pub const TEX_LOD_SIZE: usize = 0x80;
/// The textured map's tenth tag (`id(9)`): an item collection no
/// placement names and with no entries, so a test can set an item
/// reference to a tag that was not read when the map opened.
pub const TEX_SPARE_ITEM: usize = 0x1280;
/// The floor quad: `TEX_FLOOR` units square in the `z = 0` plane from the
/// origin.
pub const TEX_FLOOR: f32 = 4.0;
/// The base map's one colour (R, G, B, A) and the lightmap page's palette
/// colour (`A8R8G8B8`), index 1.
pub const TEX_BASE_RGBA: [u8; 4] = [200, 60, 20, 255];
pub const TEX_PAGE_COLOR: u32 = 0xff80_8080;
/// The BSP's checksum, which its lightmap group repeats.
const TEX_CHECKSUM: u32 = 0x00c0_ffee;

/// One resource of a raw `blkh` block: (kind, field, secondary, bytes).
type Resource = (u8, u16, u16, Vec<u8>);

fn bytes16(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn bytes32(values: impl IntoIterator<Item = u32>) -> Vec<u8> {
    values.into_iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The floor's structure section: four vertices, two triangles in one
/// triangle-list part of material 0, with positions, texture coordinates,
/// a `+z` tangent frame and lightmap coordinates.
fn floor_resources() -> Vec<Resource> {
    let mut part = vec![0u8; 0x48];
    part[0..2].copy_from_slice(&2u16.to_le_bytes()); // kind: drawn, opaque
    part[2..4].copy_from_slice(&0xbu16.to_le_bytes()); // triangle list
    part[8..10].copy_from_slice(&6u16.to_le_bytes()); // index count
    part[0xc..0xe].copy_from_slice(&1u16.to_le_bytes()); // one subpart
    let subpart = bytes16(&[0, 6, 0xffff, 0]);
    let indices = bytes16(&[0, 1, 2, 1, 3, 2]);
    let mut descriptors = vec![0u8; 0x80];
    for (i, (kind, stride)) in [(0x01u8, 12u8), (0x18, 8), (0x1b, 12), (0x1f, 4)]
        .into_iter()
        .enumerate()
    {
        descriptors[i * 0x20] = kind;
        descriptors[i * 0x20 + 1] = stride;
    }
    let s = TEX_FLOOR;
    let corners = [[0.0f32, 0.0], [s, 0.0], [0.0, s], [s, s]];
    let positions = bytes32(
        corners
            .iter()
            .flat_map(|c| [c[0], c[1], 0.0])
            .map(f32::to_bits),
    );
    let uvs = bytes32(
        [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
            .iter()
            .flatten()
            .map(|v| v.to_bits()),
    );
    // +z normal packed 11:11:10 (z = 511 in the top ten bits), binormal
    // and tangent zero.
    let frame = bytes32((0..4).flat_map(|_| [511u32 << 22, 0, 0]));
    // Two normalised i16 coordinates, both one half.
    let lightmap = bytes16(&[0x4000; 8]);
    vec![
        (0, 0, 0x48, part),
        (0, 8, 8, subpart),
        (0, 0x20, 2, indices),
        (0, 0x38, 0x20, descriptors),
        (2, 0x38, 0, positions),
        (2, 0x38, 1, uvs),
        (2, 0x38, 2, frame),
        (2, 0x38, 3, lightmap),
    ]
}

/// A `blkh` raw block of a 0x44-byte structure section and its resource
/// table. The section's block counts follow from its resources.
fn raw_block(resources: &[Resource]) -> (Vec<u8>, Vec<[u8; 16]>) {
    let mut section = vec![0u8; 0x44];
    let mut data = Vec::new();
    let mut table = Vec::new();
    for (kind, field, secondary, bytes) in resources {
        if *kind == 0 {
            let at = usize::from(*field);
            let count = (bytes.len() / usize::from(*secondary)) as u32;
            section[at..at + 4].copy_from_slice(&count.to_le_bytes());
        }
        data.extend(b"crsr");
        let mut e = [0u8; 16];
        e[0] = *kind;
        e[1] = 2;
        e[2..4].copy_from_slice(&field.to_le_bytes());
        e[4..6].copy_from_slice(&field.to_le_bytes());
        e[6..8].copy_from_slice(&secondary.to_le_bytes());
        e[8..12].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        e[12..16].copy_from_slice(&(data.len() as u32).to_le_bytes());
        data.extend(bytes);
        table.push(e);
    }
    let mut block = b"hklb".to_vec();
    block.extend((data.len() as u32).to_le_bytes());
    block.extend(&section);
    block.extend(&data);
    block.extend(b"fklb");
    (block, table)
}

/// One 4 by 4 swizzled bitmap with one image of `format` whose LOD 0 is
/// at `pixels_file` in the file, written at metadata offset `at`.
fn put_bitmap(b: &mut [u8], at: usize, format: u16, pixels_file: usize) {
    let root = META_FILE + at;
    let image = 0x50;
    put(b, root + 0x44, 1);
    put(b, root + 0x48, META + (at + image) as u32);
    let i = root + image;
    put(b, i, FourCC(*b"bitm").as_u32());
    b[i + 4..i + 6].copy_from_slice(&4u16.to_le_bytes());
    b[i + 6..i + 8].copy_from_slice(&4u16.to_le_bytes());
    b[i + 8] = 1; // depth
    b[i + 0xc..i + 0xe].copy_from_slice(&format.to_le_bytes());
    b[i + 0xe..i + 0x10].copy_from_slice(&8u16.to_le_bytes()); // swizzled
    put(b, i + 0x1c, pixels_file as u32); // LOD 0, local
    for w in [0x20, 0x24, 0x28, 0x2c, 0x30] {
        put(b, i + w, u32::MAX);
    }
    put(b, i + 0x34, TEX_LOD_SIZE as u32);
}

/// A map with one textured, lit structure BSP: one cluster, the floor
/// quad, its material's shader naming a base map, and a lightmap whose
/// group gives the cluster page 0 of a P8 bitmap through palette 0. Tags
/// 4 to 8 are the BSP, the lightmap, the shader, the base map and the
/// page. Everything is invented; the readers it goes through are
/// `native_render::load::load_world`, `base_map` and the decodes.
pub fn fixture_with_textured_bsp() -> Vec<u8> {
    let mut b = fixture();
    let mut names = NAMES.to_vec();
    names.extend(
        b"levels\\test\\floor\0levels\\test\\floor_lightmap\0levels\\test\\shaders\\floor\0levels\\test\\bitmaps\\floor\0levels\\test\\bitmaps\\floor_lightmap\0objects\\test\\spare_kit\0",
    );
    tag_names(&mut b, &names);
    let mut classes = CLASSES.to_vec();
    classes.extend([*b"sbsp", *b"ltmp", *b"shad", *b"bitm", *b"bitm", *b"itmc"]);
    let mut payloads = PAYLOADS.to_vec();
    payloads.extend([
        (TEX_SBSP, TEX_SBSP_SIZE),
        (TEX_LTMP, TEX_LTMP_SIZE),
        (TEX_SHAD, 0x100),
        (TEX_BASE, 0x100),
        (TEX_PAGE, 0x100),
        (TEX_SPARE_ITEM, 0x10),
    ]);
    tag_index(&mut b, &classes, &payloads);
    let file = |at: usize| META_FILE + at;
    let addr = |at: usize| META + at as u32;

    // The structure BSP: checksum, one material, one cluster.
    let s = file(TEX_SBSP);
    put(&mut b, s + 8, TEX_CHECKSUM);
    let (material, cluster, table) = (TEX_SBSP + 0x150, TEX_SBSP + 0x180, TEX_SBSP + 0x240);
    put(&mut b, s + 0xa4, 1);
    put(&mut b, s + 0xa8, addr(material));
    put(&mut b, file(material) + 8, FourCC(*b"shad").as_u32());
    put(&mut b, file(material) + 0xc, id(6).0);
    put(&mut b, s + 0x9c, 1);
    put(&mut b, s + 0xa0, addr(cluster));
    let (raw, entries) = raw_block(&floor_resources());
    assert!(TEX_BLOCK_FILE + raw.len() <= TEX_BASE_FILE);
    assert!(table + 16 * entries.len() <= TEX_SBSP + TEX_SBSP_SIZE);
    b[TEX_BLOCK_FILE..TEX_BLOCK_FILE + raw.len()].copy_from_slice(&raw);
    let c = file(cluster);
    b[c..c + 2].copy_from_slice(&4u16.to_le_bytes()); // vertices
    b[c + 2..c + 4].copy_from_slice(&2u16.to_le_bytes()); // triangles
    b[c + 4..c + 6].copy_from_slice(&1u16.to_le_bytes()); // parts
    b[c + 0xc..c + 0xe].copy_from_slice(&4u16.to_le_bytes());
    let info = c + 0x28;
    put(&mut b, info, TEX_BLOCK_FILE as u32);
    put(&mut b, info + 4, raw.len() as u32);
    put(&mut b, info + 8, 0x44);
    put(&mut b, info + 0xc, (raw.len() - 0x44 - 12) as u32);
    put(&mut b, info + 0x10, entries.len() as u32);
    put(&mut b, info + 0x14, addr(table));
    put(&mut b, info + 0x18, id(4).0);
    b[info + 0x1c..info + 0x1e].copy_from_slice(&0x24u16.to_le_bytes());
    b[info + 0x1f] = 1;
    put(&mut b, info + 0x20, u32::MAX);
    for (i, e) in entries.iter().enumerate() {
        let at = file(table) + 16 * i;
        b[at..at + 16].copy_from_slice(e);
    }
    let f = TEX_FLOOR;
    for (i, v) in [0.0f32, f, 0.0, f, 0.0, 0.0].into_iter().enumerate() {
        putf(&mut b, c + 0x54 + 4 * i, v);
    }
    b[c + 0x6e] = 0xff; // no sky

    // The lightmap: one group for that checksum, the page bitmap, one
    // palette, and the cluster on page 0 through palette 0.
    let l = file(TEX_LTMP);
    let (group, render, palette) = (TEX_LTMP + 0x110, TEX_LTMP + 0x180, TEX_LTMP + 0x200);
    put(&mut b, l + 0x80, 1);
    put(&mut b, l + 0x84, addr(group));
    let g = file(group);
    put(&mut b, g + 4, TEX_CHECKSUM);
    put(&mut b, g + 8, 1);
    put(&mut b, g + 0xc, addr(palette));
    put(&mut b, file(palette) + 4, TEX_PAGE_COLOR);
    put(&mut b, g + 0x18, FourCC(*b"bitm").as_u32());
    put(&mut b, g + 0x1c, id(8).0);
    put(&mut b, g + 0x28, 1);
    put(&mut b, g + 0x2c, addr(render));
    put(&mut b, file(render), 0); // page 0, palette 0

    // The shader: no template, one postprocess naming the base map.
    let h = file(TEX_SHAD);
    let (post, bitmaps) = (TEX_SHAD + 0x60, TEX_SHAD + 0xe0);
    put(&mut b, h + 4, u32::MAX);
    put(&mut b, h + 0x20, 1);
    put(&mut b, h + 0x24, addr(post));
    put(&mut b, file(post) + 4, 1);
    put(&mut b, file(post) + 8, addr(bitmaps));
    put(&mut b, file(bitmaps), id(7).0);

    // The base map, ARGB8888 (bytes B, G, R, A), and the P8 page, every
    // texel index 1.
    put_bitmap(&mut b, TEX_BASE, 11, TEX_BASE_FILE);
    let [r, g8, bl, a] = TEX_BASE_RGBA;
    let texels = [bl, g8, r, a].repeat(TEX_LOD_SIZE / 4);
    b[TEX_BASE_FILE..TEX_BASE_FILE + TEX_LOD_SIZE].copy_from_slice(&texels);
    put_bitmap(&mut b, TEX_PAGE, 18, TEX_PAGE_FILE);
    b[TEX_PAGE_FILE..TEX_PAGE_FILE + TEX_LOD_SIZE].fill(1);

    // The scenario names the BSP and its lightmap.
    block(&mut b, 0x210, 1, BSPS_AT);
    let e = sfile(BSPS_AT);
    put(&mut b, e + 0x10, FourCC(*b"sbsp").as_u32());
    put(&mut b, e + 0x14, id(4).0);
    put(&mut b, e + 0x18, FourCC(*b"ltmp").as_u32());
    put(&mut b, e + 0x1c, id(5).0);
    b
}

/// Where [`fixture_with_layered_bsp`]'s three extra tags are in the
/// metadata (past the textured map's raw block and bitmaps in the file,
/// so its metadata region grows to [`LAYERED_META_SIZE`]): the floor
/// shader's template, its one pass and the pass's vertex shader.
pub const LAYERED_STEM: usize = 0x2600;
pub const LAYERED_SPAS: usize = 0x2700;
pub const LAYERED_VRTX: usize = 0x2a00;
pub const LAYERED_META_SIZE: usize = 0x2e00;
/// The layer the floor's template draws a pass in: one of the world
/// layers the editor draws after the base look.
pub const LAYERED_LAYER: u8 = 5;
/// The pass's colour (`A8R8G8B8`): its pixel shader's final combiner
/// outputs the constant alone.
pub const LAYERED_COLOR: u32 = 0xff00_ff00;

/// [`fixture_with_textured_bsp`] with the floor's shader given a template
/// (`id(10)`) that draws one pass (`id(11)`) in layer [`LAYERED_LAYER`]
/// at the first level of detail, with one implementation: a vertex shader
/// (`id(12)`) whose world classification is the view position the
/// editor's base look takes (`native_render::base_look::world_program`:
/// the view's rows, then the game's window epilogue) and nothing else, a
/// pixel shader
/// whose final combiner outputs [`LAYERED_COLOR`], no textures, and its
/// own states: no blending, depth written, nothing culled. The base map
/// and lightmap are the textured map's, so the base look is unchanged.
/// Everything is invented.
pub fn fixture_with_layered_bsp() -> Vec<u8> {
    use shaders::psh::bits::{CHAN_ALPHA, REG_C0, final_efg, inputs};

    let mut b = fixture_with_textured_bsp();
    // The metadata region reaches the end of the file.
    put(&mut b, 0x18, LAYERED_META_SIZE as u32);
    put(&mut b, 0x1c, LAYERED_META_SIZE as u32);
    assert!(META_FILE + LAYERED_META_SIZE <= FILE);
    assert!(META_FILE + LAYERED_STEM >= TEX_PAGE_FILE + TEX_LOD_SIZE);
    let mut names = NAMES.to_vec();
    names.extend(
        b"levels\\test\\floor\0levels\\test\\floor_lightmap\0levels\\test\\shaders\\floor\0levels\\test\\bitmaps\\floor\0levels\\test\\bitmaps\\floor_lightmap\0objects\\test\\spare_kit\0levels\\test\\shaders\\layered\0levels\\test\\shaders\\layered_pass\0levels\\test\\shaders\\layered_world\0",
    );
    tag_names(&mut b, &names);
    let mut classes = CLASSES.to_vec();
    classes.extend([
        *b"sbsp", *b"ltmp", *b"shad", *b"bitm", *b"bitm", *b"itmc", *b"stem", *b"spas", *b"vrtx",
    ]);
    let mut payloads = PAYLOADS.to_vec();
    payloads.extend([
        (TEX_SBSP, TEX_SBSP_SIZE),
        (TEX_LTMP, TEX_LTMP_SIZE),
        (TEX_SHAD, 0x100),
        (TEX_BASE, 0x100),
        (TEX_PAGE, 0x100),
        (TEX_SPARE_ITEM, 0x10),
        (LAYERED_STEM, 0x100),
        (LAYERED_SPAS, 0x300),
        (LAYERED_VRTX, 0x200),
    ]);
    tag_index(&mut b, &classes, &payloads);
    let file = |at: usize| META_FILE + at;
    let addr = |at: usize| META + at as u32;
    let half = |b: &mut [u8], at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    // An index range: start in the low 9 bits, count above.
    let range = |start: u16, count: u16| start | count << 9;

    // The shader names its template, in its root and its postprocess.
    let h = file(TEX_SHAD);
    put(&mut b, h, FourCC(*b"stem").as_u32());
    put(&mut b, h + 4, id(10).0);
    put(&mut b, file(TEX_SHAD + 0x60), id(10).0);

    // The template: one level of detail whose layers 0 to 5 are listed,
    // only layer 5 drawn, its one pass the spas with one binding row
    // (no bindings).
    let s = file(LAYERED_STEM);
    for o in [0x40, 0x4c] {
        put(&mut b, s + o, FourCC(*b"shad").as_u32());
        put(&mut b, s + o + 4, u32::MAX);
    }
    let (post, lods, layers, passes, rows) = (
        LAYERED_STEM + 0x60,
        LAYERED_STEM + 0x90,
        LAYERED_STEM + 0xa0,
        LAYERED_STEM + 0xb0,
        LAYERED_STEM + 0xc0,
    );
    put(&mut b, s + 0x58, 1);
    put(&mut b, s + 0x5c, addr(post));
    for (o, count, at) in [
        (0, 1, lods),
        (8, 6, layers),
        (0x10, 1, passes),
        (0x18, 1, rows),
    ] {
        put(&mut b, file(post) + o, count);
        put(&mut b, file(post) + o + 4, addr(at));
    }
    half(&mut b, file(lods), range(0, 6));
    put(&mut b, file(lods) + 2, 1 << LAYERED_LAYER);
    half(
        &mut b,
        file(layers) + 2 * usize::from(LAYERED_LAYER),
        range(0, 1),
    );
    put(&mut b, file(passes), FourCC(*b"spas").as_u32());
    put(&mut b, file(passes) + 4, id(11).0);
    half(&mut b, file(passes) + 8, range(0, 1));

    // The pass: one implementation and its three render states.
    let p = file(LAYERED_SPAS);
    let (post, imp, states) = (
        LAYERED_SPAS + 0x30,
        LAYERED_SPAS + 0x90,
        LAYERED_SPAS + 0x1d0,
    );
    put(&mut b, p + 0x1c, 1);
    put(&mut b, p + 0x20, addr(post));
    put(&mut b, file(post), 1);
    put(&mut b, file(post) + 4, addr(imp));
    put(&mut b, file(post) + 0x10, 3);
    put(&mut b, file(post) + 0x14, addr(states));
    let i = file(imp);
    half(&mut b, i + 2, range(0, 3));
    let constant = [LAYERED_COLOR; 8];
    let pixel = shaders::PixelShaderDef {
        final_inputs_abcd: inputs(0, 0, 0, REG_C0),
        final_inputs_efg: final_efg(0, 0, REG_C0 | CHAN_ALPHA, 0),
        constant0: constant,
        final_constant0: LAYERED_COLOR,
        ..Default::default()
    }
    .to_bytes();
    b[i + 6..i + 6 + pixel.len()].copy_from_slice(&pixel);
    put(&mut b, i + 0xfc, FourCC(*b"vrtx").as_u32());
    put(&mut b, i + 0x100, id(12).0);
    {
        use gfx::state::{CullMode, RenderStateType as R};
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let slot = |t: R| {
            (0..=255u8)
                .find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t))
                .expect("a guest slot")
        };
        for (n, (t, v)) in [
            (R::AlphaBlendEnable, 0),
            (R::ZWriteEnable, 1),
            (R::CullMode, CullMode::None.xbox_value()),
        ]
        .into_iter()
        .enumerate()
        {
            let at = file(states) + 5 * n;
            b[at] = slot(t);
            put(&mut b, at + 1, v);
        }
    }

    // The vertex shader: one classification, the world's.
    let v = file(LAYERED_VRTX);
    let (classification, code_at) = (LAYERED_VRTX + 0x20, LAYERED_VRTX + 0x80);
    // The base look's own position code (`base_look::world_position`):
    // the pass computes the floor's depth as the base look does, bit for
    // bit.
    let code =
        native_render::base_look::world_program(&[]).expect("the base look's position encodes");
    assert!(code_at + code.len() <= LAYERED_VRTX + 0x200);
    put(&mut b, v + 4, 1);
    put(&mut b, v + 8, addr(classification));
    put(&mut b, file(classification) + 0xc, code.len() as u32);
    put(&mut b, file(classification) + 0x10, addr(code_at));
    b[file(code_at)..file(code_at) + code.len()].copy_from_slice(&code);
    b
}
