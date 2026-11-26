//! An invented multiplayer map the kit builds on, for this crate's tests
//! and other crates' through the `testkit` feature: the host data bundle
//! (crates/live/host-data) and the server-map strip (crates/apps/cli). No
//! input files, borrowed payloads or known disc hashes.
use std::io::Cursor;

use map::{
    CacheType, FourCC, Map, TagId,
    build::{Block, BspField, FixupKind, TagPayload, TagSet},
    writer::max_metadata_size,
};

use crate::{Config, Mode};

/// The map's name, in its header and as the kit's base name.
pub const NAME: &str = "court";

/// Invented starts: one by each flag stand on its team, one untagged in the
/// middle.
pub const STARTS: [(f32, f32, i16); 3] = [(3., 13., 0), (25., 15., 1), (14., 14., 8)];

/// `court`: two structure BSPs over the same 7x7 floor, three starts, a
/// CTF flag stand at each end, and the Elite biped the kit needs.
pub fn court() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "court", r"invented\court");
    for group in [b"scnr", b"bipd", b"weap", b"sbsp"] {
        set.add_group(FourCC(*group), None, None);
    }
    // Make the scenario the metadata anchor, so moving it would break the map.
    let scenario = set
        .add_tag(FourCC(*b"scnr"), "court", TagPayload::Empty)
        .unwrap();
    set.scenario = scenario;
    for (group, path) in [
        (b"bipd", r"objects\characters\elite\elite_mp"),
        (b"weap", r"objects\weapons\rifle\plasma_rifle\plasma_rifle"),
    ] {
        set.add_tag(FourCC(*group), path, TagPayload::Built(Block::zeroed(16)))
            .unwrap();
    }

    let mut descriptors = Block::zeroed(2 * 0x44);
    for i in 0..2u32 {
        let mut floor = crate::nav::fixture::test_floor();
        floor.put_u32(8, 100 + i);
        let id = set
            .add_tag(
                FourCC(*b"sbsp"),
                &format!("court_{i}"),
                TagPayload::Built(floor),
            )
            .unwrap();
        let section = set.add_bsp(id, None);
        let d = i * 0x44;
        for (offset, field) in [
            (0, BspField::FileOffset),
            (4, BspField::Size),
            (8, BspField::Address),
        ] {
            descriptors.fixup(d + offset, FixupKind::Bsp { section, field });
        }
        descriptors
            .tag_id(d + 0x14, id)
            .tag_id(d + 0x1c, TagId::NULL);
    }
    let mut root = Block::zeroed(0x3e0);
    let mut starts = Block::zeroed(3 * 0x34);
    for (i, (x, y, team)) in STARTS.into_iter().enumerate() {
        let at = i * 0x34;
        starts
            .put_f32(at, x)
            .put_f32(at + 4, y)
            .put_u16(at + 0x10, team as u16);
    }
    // A CTF flag stand at each end, and a flag of another type.
    let mut flags = Block::zeroed(3 * 0x20);
    for (i, (x, kind, team)) in [(2., 0u16, 0u16), (14., 1, 0), (26., 0, 1)]
        .into_iter()
        .enumerate()
    {
        let at = i * 0x20;
        flags
            .put_f32(at, x)
            .put_f32(at + 4, 14.)
            .put_u16(at + 0x10, kind)
            .put_u16(at + 0x12, team);
    }
    root.block(0x100, 3, starts)
        .block(0x118, 3, flags)
        .block(0x210, 2, descriptors);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(root);
    set.build().unwrap().0
}

/// [`court`] with the stage-one kit built on it (red home): the whole map
/// the kit package resolves to.
pub fn court_kit() -> Vec<u8> {
    crate::build(&court(), NAME, Mode::Kit, &Config::default())
        .expect("the kit builds on court")
        .bytes
}

/// `base` with its headroom cut to `room` bytes above what the kit with no
/// classes needs: a map on which a big class does not fit. The pinned
/// region's slack is a hole that tags the build adds fill for free, so a
/// filler tag takes the slack the cut adds and the base is as full as its
/// size says.
pub fn tight(base: &[u8], room: u32) -> Vec<u8> {
    let kit = crate::build(base, NAME, Mode::Kit, &Config::default()).unwrap();
    let mut map = Map::from_reader(Cursor::new(base)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let max = max_metadata_size(CacheType::Multiplayer).unwrap();
    let spare = (max - map.header().meta_size - kit.costs.tag_growth - room) & !0xfff;
    set.pinned.as_mut().unwrap().metadata_size += spare;
    set.add_group(FourCC(*b"fill"), None, None);
    set.relocate_groups = true;
    set.add_tag(
        FourCC(*b"fill"),
        r"invented\filler",
        TagPayload::Built(Block::zeroed(spare as usize - 0x200)),
    )
    .unwrap();
    set.build().unwrap().0
}

/// The donor map's label.
pub const DONOR: &str = "donor";

/// The base's plasma rifle: the donor has a tag of this name, so a class
/// that uses it reuses the base's.
pub const SHARED_WEAPON: &str = r"objects\weapons\rifle\plasma_rifle\plasma_rifle";

/// An invented donor map with three classes. Each `char` (0x20 bytes) points
/// at its biped (0x0), at [`SHARED_WEAPON`] (0x8) and carries a data
/// block (0x10) of the class's `bulk` bytes: grunt 0x20, jackal 0x20,
/// hunter 0x10000. Every biped (0x10 bytes) points at one `rail` weapon,
/// so the second class pays only for its char and biped.
pub fn donor() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, DONOR, r"invented\donor");
    for group in [b"matg", b"scnr", b"char", b"bipd", b"weap"] {
        set.add_group(FourCC(*group), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals",
            TagPayload::Built(Block::zeroed(0x284)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            DONOR,
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let mut weapon = |name: &str| {
        set.add_tag(
            FourCC(*b"weap"),
            name,
            TagPayload::Built(Block::zeroed(0x10)),
        )
        .unwrap()
    };
    let shared = weapon(SHARED_WEAPON);
    let rail = weapon(r"donor\weapons\rail");
    for (name, bulk) in [("grunt", 0x20usize), ("jackal", 0x20), ("hunter", 0x10000)] {
        let mut unit = Block::zeroed(0x10);
        unit.tag_ref(0, rail);
        let unit = set
            .add_tag(
                FourCC(*b"bipd"),
                &format!(r"donor\units\{name}"),
                TagPayload::Built(unit),
            )
            .unwrap();
        let mut root = Block::zeroed(0x20);
        root.tag_ref(0, unit)
            .tag_ref(8, shared)
            .data(0x10, Block::new(vec![7; bulk]).aligned(1));
        set.add_tag(
            FourCC(*b"char"),
            &format!(r"donor\characters\{name}"),
            TagPayload::Built(root),
        )
        .unwrap();
    }
    set.build().unwrap().0
}

/// The layouts of [`donor`]'s three groups.
pub fn donor_schema() -> tags::schema::Schema {
    use std::collections::BTreeMap;
    use tags::schema::{Field, Schema, Struct};
    let bytes = Struct {
        size: 1,
        align: 1,
        fields: BTreeMap::new(),
    };
    let group = |size, fields| Struct {
        size,
        align: 4,
        fields,
    };
    Schema {
        groups: BTreeMap::from([
            (
                FourCC(*b"char"),
                group(
                    0x20,
                    BTreeMap::from([
                        (0, Field::TagRef),
                        (8, Field::TagRef),
                        (0x10, Field::Block(Box::new(bytes))),
                    ]),
                ),
            ),
            (
                FourCC(*b"bipd"),
                group(0x10, BTreeMap::from([(0, Field::TagRef)])),
            ),
            (FourCC(*b"weap"), group(0x10, BTreeMap::new())),
        ]),
    }
}

/// The three classes of [`donor`], in keep order: the Jackal stands in for a
/// Flood class, the others are Covenant.
pub fn donor_classes() -> Vec<crate::ClassSpec> {
    ["grunt", "jackal", "hunter"]
        .into_iter()
        .map(|name| crate::ClassSpec {
            name: name.into(),
            donor: 0,
            path: format!(r"donor\characters\{name}"),
            faction: if name == "jackal" {
                firefight::Faction::Flood
            } else {
                firefight::Faction::Covenant
            },
        })
        .collect()
}
