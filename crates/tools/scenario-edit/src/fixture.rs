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
