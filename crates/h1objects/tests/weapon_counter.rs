use h1convert::shad::{Bound, Fixed, ILLUM_DETAIL, Library, ONE_ADD_ILLUM};
use h1convert::sky_shaders::{BLEND, BLEND_ADD, MAPS, SCHI_MAP};
use h1convert::textures::ModelTemplate;
use h1map::bitmap;
use h1map::testing::{BASE, SCNR_ID, Spec, build_with_extra_tags, group, put};
use h1map::{H1Map, HEADER_LEN};
use h1models::h1::MODE_HEADER;
use h1objects::base::Base;
use h1objects::convert::{Converter, local, local_index};
use h1objects::h1::{MAGAZINE_SIZE, WEAP_SIZE, WEAPON_PRIMARY_AMMUNITION, Weapon};
use map::{FourCC, TagId};
use tags::defs::{Defs, Record, Target, Val, from_built};
use tags::object::Metadata;

fn counter_map() -> H1Map {
    const LOADED: u8 = 24;
    const FUNCTION_SIZE: usize = 0x168;
    const SHADER_REF_SIZE: usize = 0x20;
    const CHICAGO_SIZE: usize = 0x6c;
    const DIGITS: usize = 10;

    let spec = |group, name, size| Spec {
        group,
        name,
        parent: None,
        data: Some(vec![0; size]),
    };
    let mut built = build_with_extra_tags(
        &Default::default(),
        vec![
            spec(b"weap", "weapon", WEAP_SIZE + FUNCTION_SIZE + MAGAZINE_SIZE),
            spec(b"mode", "world", MODE_HEADER + 2 * SHADER_REF_SIZE),
            spec(b"mode", "first_person", MODE_HEADER + 2 * SHADER_REF_SIZE),
            spec(b"schi", "counter", CHICAGO_SIZE + SCHI_MAP.0),
            spec(
                b"bitm",
                "digits",
                bitmap::HEADER_LEN + DIGITS * bitmap::IMAGE_LEN,
            ),
            spec(
                b"weap",
                "short_magazine",
                WEAP_SIZE + FUNCTION_SIZE + MAGAZINE_SIZE,
            ),
        ],
    );
    let first = built.data_at.len() - 6;
    let id = |i: usize| SCNR_ID + (first + i) as u32;
    let at = |i: usize| built.data_at[first + i].unwrap();
    let address = |offset: usize| BASE + (offset - HEADER_LEN) as u32;
    let pixel_offset = built.file.len() as u32;
    built.file.resize(built.file.len() + 8 * DIGITS, 0);
    let file_len = built.file.len() as u32;
    let b = &mut built.file;
    put(b, 8, file_len);

    for (weapon, loaded) in [(at(0), LOADED), (at(5), LOADED / 2)] {
        let function = weapon + WEAP_SIZE;
        let magazine = function + FUNCTION_SIZE;
        put(b, weapon, 2); // obje type: weapon
        put(b, weapon + 0x28, group(b"mode"));
        put(b, weapon + 0x34, id(1));
        put(b, weapon + 0x45c, group(b"mode"));
        put(b, weapon + 0x468, id(2));
        put(b, weapon + 0x330, u32::from(WEAPON_PRIMARY_AMMUNITION));
        put(b, weapon + 0x158, 1);
        put(b, weapon + 0x15c, address(function));
        put(b, function + 0xc, 1); // A out scales by A in
        put(b, function + 0x2c, 1.0f32.to_bits());
        put(b, weapon + 0x4f0, 1);
        put(b, weapon + 0x4f4, address(magazine));
        put(b, magazine + 0xa, u32::from(loaded));
    }

    for model in [at(1), at(2)] {
        let shaders = model + MODE_HEADER;
        put(b, model + 0xdc, 2);
        put(b, model + 0xe0, address(shaders));
        for permutation in 0..2 {
            let entry = shaders + permutation * SHADER_REF_SIZE;
            put(b, entry, group(b"schi"));
            put(b, entry + 0xc, id(3));
            put(b, entry + 0x10, permutation as u32);
        }
    }

    let shader = at(3);
    let map = shader + CHICAGO_SIZE;
    b[shader + h1convert::shad::CHICAGO_COUNTER_LIMIT] = LOADED;
    b[shader + h1convert::shad::CHICAGO_FLAGS] = h1convert::shad::CHICAGO_NUMERIC;
    put(b, shader + BLEND, u32::from(BLEND_ADD));
    put(b, shader + MAPS, 1);
    put(b, shader + MAPS + 4, address(map));
    put(b, map + SCHI_MAP.1, group(b"bitm"));
    put(b, map + SCHI_MAP.1 + 0xc, id(4));
    put(b, map + SCHI_MAP.2, 1.0f32.to_bits());
    put(b, map + SCHI_MAP.2 + 4, 1.0f32.to_bits());

    let bitmap_at = at(4);
    let images = bitmap_at + bitmap::HEADER_LEN;
    put(b, bitmap_at + bitmap::IMAGES, DIGITS as u32);
    put(b, bitmap_at + bitmap::IMAGES + 4, address(images));
    for i in 0..DIGITS {
        let image = images + i * bitmap::IMAGE_LEN;
        put(b, image, group(b"bitm"));
        put(b, image + 4, 4 | (4 << 16)); // width and height
        put(b, image + 8, 1); // depth 1, 2D
        put(
            b,
            image + 0xc,
            u32::from(bitmap::PixelFormat::Dxt1.raw())
                | (u32::from(bitmap::FLAG_COMPRESSED) << 16),
        );
        put(b, image + 0x18, pixel_offset + 8 * i as u32);
        put(b, image + 0x1c, 8);
    }
    H1Map::from_decompressed(built.file).unwrap()
}

fn counter_base() -> Base {
    let fixed = Bound {
        id: TagId(0xe200_0001),
        scale: [1.0, 1.0],
        lod_offset: 0.0,
    };
    Base {
        meta: Metadata {
            base: 0,
            bytes: Vec::new(),
        },
        tags: Vec::new(),
        model_shader: Some(ModelTemplate {
            shader: TagId(0xe200_0002),
            name: "stand-in".into(),
            library: Library::new(
                &[
                    (ILLUM_DETAIL.stem, TagId(0xe200_0003)),
                    (ONE_ADD_ILLUM.stem, TagId(0xe200_0004)),
                ],
                &[(Fixed::ColorWhite, fixed), (Fixed::DefaultDetail, fixed)],
            ),
        }),
        materials: Vec::new(),
    }
}

fn shader(conv: &Converter<'_>, name: &str) -> Option<TagId> {
    conv.tree
        .tags
        .iter()
        .position(|(n, c)| n == name && c.group == FourCC(*b"shad"))
        .map(local)
}

fn weapon_materials(conv: &Converter<'_>, weapon: TagId) -> [[TagId; 2]; 2] {
    let record = |id| {
        let c = &conv.tree.tags[local_index(id).unwrap()].1;
        let def = Defs::builtin().group(c.group).unwrap();
        from_built(def, &c.root).unwrap()
    };
    let reference = |r: &Record, field: &str| match r.at(field) {
        Some(Val::Ref {
            target: Target::Id(id),
            ..
        }) => *id,
        other => panic!("{field}: {other:?}"),
    };
    let weapon = record(weapon);
    let hlmt = record(reference(&weapon, "item.obje.model"));
    let world = reference(&hlmt, "render_model");
    let first_person = reference(&weapon, "first_person[0].model");
    assert_eq!(
        weapon.at("item.obje.predicted_resources[0].tag"),
        Some(&Val::Datum(Target::Id(world))),
    );
    [world, first_person].map(|id| {
        let model = record(id);
        std::array::from_fn(|i| reference(&model, &format!("materials[{i}].shader")))
    })
}

#[test]
fn both_weapon_models_use_the_same_ammunition_digits() {
    let h1 = counter_map();
    let base = counter_base();
    let weapon = h1.find(h1map::Group::new(b"weap"), "weapon").unwrap();
    assert_eq!(Weapon::read(&h1, weapon).unwrap().rounds_counter(), Some(24));
    let mut conv = Converter::new(&h1, &base);
    let id = conv.object(weapon).unwrap();

    assert!(shader(&conv, "counter").is_none());
    let digits = ["counter_ones", "counter_tens"].map(|name| shader(&conv, name).unwrap());
    assert_eq!(weapon_materials(&conv, id), [digits; 2]);
}

#[test]
fn shared_weapon_models_use_their_own_magazine_limit() {
    let h1 = counter_map();
    let base = counter_base();
    for order in [["short_magazine", "weapon"], ["weapon", "short_magazine"]] {
        let mut conv = Converter::new(&h1, &base);
        let converted = order.map(|name| {
            let tag = h1.find(h1map::Group::new(b"weap"), name).unwrap();
            (name, conv.object(tag).unwrap())
        });
        let static_shader = shader(&conv, "counter").unwrap();
        let digits = ["counter_ones", "counter_tens"].map(|name| shader(&conv, name).unwrap());
        for (name, id) in converted {
            let want = if name == "short_magazine" {
                [static_shader; 2]
            } else {
                digits
            };
            assert_eq!(weapon_materials(&conv, id), [want; 2], "{name} in {order:?}");
        }
    }
}
