//! An invented multiplayer map the kit goes into (feature `fixture`, and
//! this crate's tests): no game data, so other crates' tests can put the
//! kit into a map and look at what it made (Studio's `model.*`).

use map::CacheType;
use map::FourCC;
use map::build::{Block, TagPayload, TagSet};
use tags::defs::{self, Defs, Val};

use crate::{kit, texture};

/// The class the fixtures' kits are built with: the fixture map has no
/// animation graphs to read one from.
pub fn fixture_class() -> (String, String) {
    ("pistol".to_owned(), "hp".to_owned())
}

/// The fixture pistol's first-person models and graphs, one pair a
/// player character.
pub const FIXTURE_FIRST_PERSON: [(&str, &str); 2] = [
    (r"invented\fp\pistol_chief", r"invented\fp\pistol_chief"),
    (r"invented\fp\pistol_elite", r"invented\fp\pistol_elite"),
];

/// The group of the fixture's padding tag: one nothing reads.
const PAD_GROUP: &[u8; 4] = b"bloc";

/// A multiplayer map of invented tags holding what the kit reads from a
/// map: a shader template with its default bitmaps, and globals naming
/// materials. `pad` adds a tag of that many bytes.
pub fn fixture(pad: usize) -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "court", r"invented\court");
    for group in [b"scnr", b"matg", b"stem", b"bitm", b"mode", b"jmad", PAD_GROUP] {
        set.add_group(FourCC(*group), None, None);
    }
    set.add_group(FourCC(*b"weap"), Some(FourCC(*b"item")), Some(FourCC(*b"obje")));
    let globals = {
        let mut names = Block::zeroed(2 * 0xb4);
        names.string_id(0, "soft_organic_flesh");
        names.string_id(0xb4, kit::MATERIAL);
        let mut root = Block::zeroed(0x284);
        root.block(0x150, 2, names);
        root
    };
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            r"globals\globals",
            TagPayload::Built(globals),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\court",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    set.add_tag(
        FourCC(*b"stem"),
        kit::LAYOUTS[0].stem,
        TagPayload::Built(Block::zeroed(0x60)),
    )
    .unwrap();
    let grey = texture::Rgba {
        width: 4,
        height: 4,
        pixels: vec![128; 64],
    };
    for path in [
        r"shaders\default_bitmaps\bitmaps\default_vector",
        r"shaders\default_bitmaps\bitmaps\default_detail",
        r"shaders\default_bitmaps\bitmaps\color_white",
        r"shaders\default_bitmaps\bitmaps\default_additive_cube_map",
        r"rasterizer\linear_corner_fade",
    ] {
        texture::bitmap(&grey)
            .unwrap()
            .install(&mut set, path)
            .unwrap();
    }
    // The map's own pistol, whose first-person view the stick takes: one
    // element a player character. A rifle of another class before it.
    let mut fp = Vec::new();
    for (i, (model, graph)) in FIXTURE_FIRST_PERSON.iter().enumerate() {
        let model = set
            .add_tag(FourCC(*b"mode"), model, TagPayload::Built(Block::zeroed(0x10)))
            .unwrap();
        let graph = set
            .add_tag(FourCC(*b"jmad"), graph, TagPayload::Built(Block::zeroed(0x10)))
            .unwrap();
        fp.push((i as u32, model, graph));
    }
    // Built from the weap definition, as the kit builds its own, so every
    // reference left unset is a null one (-1), as in a real map: a zeroed
    // reference names class 0 and datum 0, which no field allows.
    let weap = Defs::builtin()
        .group(FourCC(*b"weap"))
        .expect("a weap definition");
    let element = weap.child("first_person").expect("weap first_person");
    for (path, class, elements) in [
        (r"invented\rifle", ("rifle", "ar"), &fp[..1]),
        (r"invented\pistol", ("pistol", "hp"), &fp[..]),
    ] {
        let mut r = defs::default_record(weap);
        r.set(
            "item.obje.object_type",
            Val::Enum(h1objects::arsenal::WEAPON_TYPE),
        )
        .unwrap();
        r.set("weapon_class", Val::text(class.0)).unwrap();
        r.set("weapon_name", Val::text(class.1)).unwrap();
        let mut list = Vec::new();
        for (_, model, graph) in elements {
            let mut x = defs::default_record(element);
            x.set("model", Val::tag(FourCC(*b"mode"), *model)).unwrap();
            x.set("animations", Val::tag(FourCC(*b"jmad"), *graph))
                .unwrap();
            list.push(x);
        }
        r.set("first_person", Val::Block(list)).unwrap();
        let root = defs::to_built(weap, &r, &mut |_| None).expect("the fixture weap");
        set.add_tag(FourCC(*b"weap"), path, TagPayload::Built(root))
            .unwrap();
    }
    if pad > 0 {
        set.add_tag(
            FourCC(*PAD_GROUP),
            "pad",
            TagPayload::Built(Block::zeroed(pad)),
        )
        .unwrap();
    }
    set.build().expect("the fixture builds").0
}

/// The fixture with the kit in it, built as the game serves a map
/// ([`crate::inject::add_pogo_with`], the class given).
pub fn kitted() -> crate::inject::Added {
    let given = crate::inject::Given {
        class: Some(fixture_class()),
        tag_memory: None,
    };
    crate::inject::add_pogo_with(&fixture(0), "court", &given).expect("the kit goes into the fixture")
}
