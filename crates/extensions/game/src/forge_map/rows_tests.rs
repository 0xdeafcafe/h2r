//! `forge_map::rows` over invented guest memory: each kind's things, the
//! prints, and the blocks that do not read.
use guest_layout::testing::FakeGuest;
use forge::BlockKind;
use forge_doc::{F32, TagKey, Value};
use map::scenario_layout as layout;

use super::rows::{self, MapRows, MapThing, ThingKind};

const SCENARIO: u32 = 0x8100_0000;
const SHELF: u32 = 0xe001_0001;
const CRATE: u32 = 0xe001_0002;
const RIFLE: u32 = 0xe001_0003;

fn key(group: &str, path: &str) -> TagKey {
    TagKey {
        group: group.into(),
        path: path.into(),
    }
}

fn names(datum: u32) -> Option<TagKey> {
    match datum {
        SHELF => Some(key("scen", "test\\shelf")),
        CRATE => Some(key("scen", "test\\crate")),
        RIFLE => Some(key("weap", "test\\rifle")),
        _ => None,
    }
}

fn f(v: f32) -> Value {
    Value::Float(F32::new(v).unwrap())
}

fn v3(v: [f32; 3]) -> Value {
    Value::Vec3(v.map(|c| F32::new(c).unwrap()))
}

/// Write `value` into the field `name` of the row at `at`.
fn put(g: &mut FakeGuest, at: u32, block: BlockKind, name: &str, value: &Value) {
    let field = layout::block(block).field(name).unwrap();
    let at = at + field.offset;
    match value {
        Value::Int(n) => match field.ty.size() {
            1 => g.put_u8(at, *n as u8),
            2 => g.put_u16(at, *n as u16),
            _ => g.put_u32(at, *n as u32),
        },
        Value::Float(n) => g.put_f32(at, n.get()),
        Value::Vec3(v) => {
            for (i, c) in v.iter().enumerate() {
                g.put_f32(at + 4 * i as u32, c.get());
            }
            g
        }
        other => panic!("{other:?}"),
    };
}

/// Point a block of the scenario root at `rows` rows at `address`.
fn block(g: &mut FakeGuest, kind: BlockKind, rows: u32, address: u32) -> u32 {
    let def = layout::block(kind);
    g.map(address, rows.max(1) * def.stride)
        .put_u32(SCENARIO + def.offset, rows)
        .put_u32(SCENARIO + def.offset + 4, address);
    address
}

/// An invented scenario: two scenery placements over a two-entry palette,
/// two starting points, a goal and two teleporter flags, and two spawners.
fn guest() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(SCENARIO, 0x400);
    let palette = layout::block(BlockKind::Scenery).palette.as_ref().unwrap();
    g.map(0x8200_0000, 0x100)
        .put_u32(SCENARIO + palette.offset, 2)
        .put_u32(SCENARIO + palette.offset + 4, 0x8200_0000)
        .put_u32(0x8200_0004, SHELF)
        .put_u32(0x8200_0000 + palette.stride + 4, CRATE);

    let scen = block(&mut g, BlockKind::Scenery, 2, 0x8210_0000);
    let stride = layout::block(BlockKind::Scenery).stride;
    for (i, (palette, x)) in [(1, 3.5), (0, -2.0)].into_iter().enumerate() {
        let at = scen + i as u32 * stride;
        let s = BlockKind::Scenery;
        put(&mut g, at, s, "palette index", &Value::Int(palette));
        put(&mut g, at, s, "name index", &Value::Int(-1));
        put(&mut g, at, s, "position", &v3([x, 1.0, 0.5]));
        put(&mut g, at, s, "rotation", &v3([0.25, 0.0, 0.0]));
        put(&mut g, at, s, "scale", &f(1.0));
    }

    let s = BlockKind::StartingLocations;
    let starts = block(&mut g, s, 2, 0x8220_0000);
    for i in 0..2 {
        let at = starts + i * layout::block(s).stride;
        put(&mut g, at, s, "position", &v3([i as f32, 2.0, 3.0]));
        put(&mut g, at, s, "team", &Value::Int(i64::from(i)));
    }

    let s = BlockKind::NetgameFlags;
    let flags = block(&mut g, s, 3, 0x8230_0000);
    for (i, (kind, channel)) in [(0, 0), (7, 4), (8, 4)].into_iter().enumerate() {
        let at = flags + i as u32 * layout::block(s).stride;
        put(&mut g, at, s, "position", &v3([9.0, i as f32, 1.0]));
        put(&mut g, at, s, "type", &Value::Int(kind));
        put(&mut g, at, s, "identifier", &Value::Int(channel));
    }

    let s = BlockKind::NetgameEquipment;
    let items = block(&mut g, s, 2, 0x8240_0000);
    let item = layout::block(s).field("item").unwrap();
    for i in 0..2 {
        let at = items + i * layout::block(s).stride;
        put(
            &mut g,
            at,
            s,
            "respawn time",
            &Value::Int(30 + i64::from(i)),
        );
        // The first spawner has no item; the second names the rifle.
        g.put_u32(at + item.offset + 4, if i == 0 { u32::MAX } else { RIFLE });
    }
    g
}

fn read(g: &FakeGuest) -> MapRows {
    rows::read(g, SCENARIO, &names)
}

#[test]
fn each_kind_of_thing_is_listed_with_the_layouts_fields() {
    let rows = read(&guest());
    assert!(rows.left_out.is_empty(), "{:?}", rows.left_out);
    let things = rows.things();
    let kinds = |k: ThingKind| things.iter().filter(|t| t.kind == k).count();
    assert_eq!(kinds(ThingKind::Object), 2);
    assert_eq!(kinds(ThingKind::StartingPoint), 2);
    assert_eq!(kinds(ThingKind::Goal), 1);
    assert_eq!(kinds(ThingKind::Teleporter), 2);
    assert_eq!(kinds(ThingKind::ItemSpawner), 2);
    assert_eq!(things.len(), 9);

    // Fields come by the layout's names, in its order, and a palette index
    // is the key of the entry it selects.
    let first = things
        .iter()
        .find(|t| t.block == BlockKind::Scenery && t.index == 0)
        .unwrap();
    let listed: Vec<_> = first.fields.iter().map(|(n, _)| *n).collect();
    let expected: Vec<_> = layout::block(BlockKind::Scenery)
        .fields
        .iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(listed, expected);
    assert_eq!(
        first.fields[0].1,
        Value::Tag(Some(key("scen", "test\\crate")))
    );
    assert_eq!(first.fields[3].1, v3([3.5, 1.0, 0.5]));

    // A null item reference is none; a named one is its key.
    let spawners: Vec<_> = things
        .iter()
        .filter(|t| t.kind == ThingKind::ItemSpawner)
        .collect();
    let item = |t: &&MapThing| {
        t.fields
            .iter()
            .find(|(n, _)| *n == "item")
            .unwrap()
            .1
            .clone()
    };
    assert_eq!(item(&spawners[0]), Value::Tag(None));
    assert_eq!(
        item(&spawners[1]),
        Value::Tag(Some(key("weap", "test\\rifle")))
    );

    // The raw row is kept, a stride of it, to put back.
    let row = rows.row(BlockKind::Scenery, 1).unwrap();
    assert_eq!(
        row.raw.len(),
        layout::block(BlockKind::Scenery).stride as usize
    );
}

#[test]
fn a_print_is_the_digest_of_the_typed_fields_alone() {
    let rows = read(&guest());
    let expected = forge_doc::print(
        BlockKind::Scenery,
        &[
            Value::Tag(Some(key("scen", "test\\crate"))),
            Value::Int(-1),
            Value::Int(0),
            v3([3.5, 1.0, 0.5]),
            v3([0.25, 0.0, 0.0]),
            f(1.0),
        ],
    )
    .unwrap();
    assert_eq!(rows.row(BlockKind::Scenery, 0).unwrap().print, expected);
    assert_ne!(
        rows.row(BlockKind::Scenery, 0).unwrap().print,
        rows.row(BlockKind::Scenery, 1).unwrap().print
    );

    // A moved entry prints differently; its neighbour does not.
    let mut moved = guest();
    put(
        &mut moved,
        0x8210_0000,
        BlockKind::Scenery,
        "position",
        &v3([3.5, 1.0, 0.75]),
    );
    let after = read(&moved);
    assert_ne!(after.row(BlockKind::Scenery, 0).unwrap().print, expected);
    assert_eq!(
        after.row(BlockKind::Scenery, 1).unwrap().print,
        rows.row(BlockKind::Scenery, 1).unwrap().print
    );
}

#[test]
fn a_block_that_does_not_read_is_left_out_by_name_and_the_rest_stay() {
    // An unmapped address, a count past the engine's limit, a palette index
    // outside the palette and a NaN each leave their own block out.
    let mut g = guest();
    let vehicles = layout::block(BlockKind::Vehicles).offset;
    g.put_u32(SCENARIO + vehicles, 3)
        .put_u32(SCENARIO + vehicles + 4, 0x9000_0000);
    g.put_u32(SCENARIO + layout::block(BlockKind::Crates).offset, 100_000);
    put(
        &mut g,
        0x8210_0000,
        BlockKind::Scenery,
        "palette index",
        &Value::Int(5),
    );
    g.put_u32(0x8220_0000, f32::NAN.to_bits());
    let rows = read(&g);
    let left: Vec<_> = rows
        .left_out
        .iter()
        .map(|l| l.split(':').next().unwrap())
        .collect();
    assert_eq!(
        left,
        ["scenery", "vehicles", "crates", "starting_locations"]
    );
    assert!(rows.left_out[0].contains("row 0"));
    assert!(rows.left_out[0].contains("outside the palette"));
    assert!(rows.left_out[1].contains("does not read"));
    assert!(rows.left_out[2].contains("more than"));
    assert!(rows.left_out[3].contains("not a finite number"));
    assert!(rows.blocks.contains_key(&BlockKind::NetgameFlags));
    assert!(!rows.blocks.contains_key(&BlockKind::Scenery));
    assert_eq!(rows.things().len(), 5);

    // A tag the map does not hold leaves a block out too.
    let mut g = guest();
    g.put_u32(0x8200_0004, 0xe0ff_0000);
    let rows = read(&g);
    assert!(rows.left_out[0].starts_with("scenery: palette entry 0"));
}
