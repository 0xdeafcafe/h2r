use super::{Base, Converter, Group, h1_map, h2_map, local};
use h1convert::shad::{ONE_ADD_ILLUM, program::model_effects::PLASMA_VERTEX};
use h1map::bitmap::Bitmap;
use map::{FourCC, TagId};
use tags::defs::{Defs, Record, Target, Val, from_built};
use tags::effects::function::Function;

fn reference(record: &Record, path: &str) -> TagId {
    match record.at(path) {
        Some(
            Val::Ref {
                target: Target::Id(id),
                ..
            }
            | Val::Datum(Target::Id(id)),
        ) => *id,
        other => panic!("{path}: {other:?}"),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn six_transparent_materials_bind_the_source_images_and_programs() {
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    for level in ["a50", "d20", "c10", "c20", "c40", "d40"] {
        let h1 = h1_map(level);
        let door = matches!(level, "a50" | "d20");
        let (group, model, shader, bitmap) = if door {
            (
                b"mach",
                r"levels\a50\devices\small door\small door",
                r"levels\a50\devices\small door\shaders\door lock symbols",
                r"rasterizer\default 2d",
            )
        } else {
            (
                b"bipd",
                r"characters\sentinel\sentinel",
                r"characters\sentinel\shaders\sentinel_shield",
                r"characters\elite\bitmaps\plasma shield noise",
            )
        };
        let mut conv = Converter::new(&h1, &base);
        let object = h1.find(Group::new(group), model).unwrap();
        conv.object(object)
            .unwrap_or_else(|e| panic!("{level}: {e}"));
        let named = |g: &[u8; 4], name: &str| {
            conv.tree
                .tags
                .iter()
                .position(|(n, c)| n == name && c.group == FourCC(*g))
                .unwrap_or_else(|| panic!("{level}: missing {name}"))
        };
        let record = |i: usize| {
            let c = &conv.tree.tags[i].1;
            from_built(Defs::builtin().group(c.group).unwrap(), &c.root).unwrap()
        };
        let image = named(b"bitm", bitmap);
        let shad = named(b"shad", shader);
        assert_eq!(
            reference(&record(named(b"mode", model)), "materials[1].shader"),
            local(shad)
        );
        let instance = record(shad);
        assert_eq!(
            reference(&instance, "postprocess[0].bitmaps[0].bitmap"),
            local(image)
        );
        let source = Bitmap::read(&h1, h1.find(Group::new(b"bitm"), bitmap).unwrap()).unwrap();
        let pixels = source.pixels(&h1, 0).unwrap();
        let rebuilt = &conv.tree.tags[image].1.raw[0].0;
        if door {
            assert_eq!(
                reference(&instance, "template"),
                base.find(b"stem", ONE_ADD_ILLUM.stem).unwrap()
            );
            assert!(pixels[..64].chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
            assert_eq!(&rebuilt[..64], &pixels[..64]);
            if level == "d20" {
                let bump = named(
                    b"bitm",
                    r"levels\a50\devices\small door\bitmaps\floor level symbols_alpha_test_bump",
                );
                let image_record = record(bump);
                let Some(Val::Block(images)) = image_record.get("images") else {
                    panic!("missing cutout bump images");
                };
                assert_eq!(images.len(), 4);
                assert_eq!(conv.tree.tags[bump].1.raw.len(), 4);
            }
            continue;
        }
        assert_eq!(
            reference(&instance, "postprocess[0].bitmaps[1].bitmap"),
            local(image)
        );
        let image_record = record(image);
        for field in ["width", "height", "depth"] {
            assert_eq!(
                image_record.at(&format!("images[0].{field}")),
                Some(&Val::Int(32))
            );
        }
        assert_eq!(image_record.at("images[0].kind"), Some(&Val::Enum(1)));
        assert_eq!(image_record.at("images[0].format"), Some(&Val::Int(3)));
        assert_eq!(image_record.at("images[0].mip_count"), Some(&Val::Int(5)));
        assert_eq!(rebuilt.as_slice(), pixels);
        let stem = named(b"stem", shader);
        let pass = named(b"spas", shader);
        assert_eq!(reference(&instance, "template"), local(stem));
        assert_eq!(
            reference(&record(stem), "postprocess[0].passes[0].pass"),
            local(pass)
        );
        assert_eq!(
            reference(
                &record(pass),
                "postprocess[0].implementations[0].vertex_shader"
            ),
            base.find(b"vrtx", PLASMA_VERTEX).unwrap()
        );
        let pp = "postprocess[0]";
        let h1_shader = h1.find(Group::new(b"spla"), shader).unwrap();
        let data = h1.tag_data(h1_shader).unwrap();
        let value =
            |offset: usize| f32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let endpoint = |at: usize| [value(at + 4), value(at + 8), value(at + 12), value(at)];
        let facing = endpoint(0x60);
        let parallel = endpoint(0x70);
        let grazing: [f32; 4] = std::array::from_fn(|i| facing[i] - parallel[i]);
        for (k, rgba) in [(6, facing), (7, grazing)] {
            assert_eq!(
                instance.at(&format!("{pp}.vectors[{k}].vector")),
                Some(&Val::floats(&rgba))
            );
        }
        assert_eq!(grazing[3], 0.0);
        assert_eq!(
            reference(&instance, "bitmap_images[0].bitmap"),
            local(image)
        );
        assert!(instance.at("bitmap_images[1].bitmap").is_none());
        for (k, target) in [0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x6a, 0x6b]
            .into_iter()
            .enumerate()
        {
            assert_eq!(
                instance.at(&format!("{pp}.animations[{k}].target")),
                Some(&Val::Int(target))
            );
            assert_eq!(
                instance.at(&format!("{pp}.animations[{k}].property_range")),
                Some(&Val::Int(k as i64))
            );
            assert_eq!(
                instance.at(&format!("{pp}.property_ranges[{k}].property_refs")),
                Some(&Val::Int(0x200 + k as i64))
            );
        }
        for (k, expected) in [2.0f32, 0.0, -0.3, 0.0, 2.0, 0.5].into_iter().enumerate() {
            let Some(Val::Float(period)) = instance.at(&format!("{pp}.properties[{k}].input_b"))
            else {
                panic!("period")
            };
            let Some(Val::Data(blob)) = instance.at(&format!("{pp}.properties[{k}].function"))
            else {
                panic!("slide")
            };
            let f = Function::parse(blob).unwrap();
            let rate = (f.range[1] - f.range[0]) / period;
            assert!(
                (rate - expected / if k < 3 { 2.1 } else { 3.2 }).abs() < 1e-6,
                "{level} axis {k}"
            );
        }
        for k in [6, 7] {
            assert_eq!(
                instance.at(&format!("{pp}.properties[{k}].input")),
                Some(&Val::text("a_out"))
            );
        }
        let biped = record(named(b"bipd", model));
        assert_eq!(
            biped.at("unit.obje.functions[0].string_4"),
            Some(&Val::text("current_shield_damage"))
        );
        assert_eq!(
            biped.at("unit.obje.functions[0].output"),
            Some(&Val::text("a_out"))
        );
    }
}
