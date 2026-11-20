use super::{Base, Converter, Group, h1_map, h2_map, local};

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn campaign_null_base_model_materials_use_their_own_shaders() {
    use h1convert::shad::{Fixed, TEX_BUMP};
    use h1objects::convert::local_index;
    use map::FourCC;
    use tags::defs::{Defs, Target, Val, from_built};

    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    let white = base.find(b"bitm", Fixed::ColorWhite.path()).unwrap();
    let opaque = base.find(b"stem", TEX_BUMP.stem).unwrap();
    let cases: &[(&[&str], &str, usize, &str, &[usize])] = &[
        (
            &["a10", "d40"],
            r"levels\a10\devices\alert_light\alert_light",
            1,
            r"levels\a10\shaders\light siren",
            &[7],
        ),
        (
            &["a10", "d40"],
            r"levels\a10\devices\doors\door blast closed\door blast closed",
            2,
            r"levels\a10\shaders\obj-light lock",
            &[7, 2],
        ),
        (
            &["a10", "d40"],
            r"levels\a10\devices\doors\door blast half\door blast half",
            2,
            r"levels\a10\shaders\obj-light lock",
            &[7, 2],
        ),
        (
            &["a10", "d40"],
            r"levels\a10\devices\doors\door blast\door blast",
            6,
            r"levels\a10\shaders\obj-light lock",
            &[7, 2],
        ),
        (
            &["a10", "d40"],
            r"levels\a10\devices\doors\door jeff tube\door jeff tube",
            3,
            r"levels\a10\shaders\obj-light lock",
            &[7, 2],
        ),
        (
            &["a10"],
            r"levels\a10\devices\shield charge\shield charge",
            4,
            r"levels\a10\shaders\obj-shield charge",
            &[7],
        ),
        (
            &["d40"],
            r"levels\a10\devices\signal light\signal light",
            0,
            r"levels\a10\shaders\light signal",
            &[7, 2],
        ),
        (
            &["a30"],
            r"scenery\waterfalls\waterfall_emitter\waterfall_emitter",
            0,
            r"scenery\waterfalls\spray_emitter_medium\shader\invisible",
            &[],
        ),
        (
            &["b30", "d20"],
            r"scenery\debris\rocks_smoldering\rocks_smoldering",
            0,
            r"scenery\debris\rocks_smoldering\shaders\rocks_smoldering",
            &[],
        ),
    ];
    let mut checked = 0;
    for level in ["a10", "a30", "b30", "d20", "d40"] {
        let h1 = h1_map(level);
        let mut conv = Converter::new(&h1, &base);
        for &(levels, model, slot, shader, layers) in cases {
            if !levels.contains(&level) {
                continue;
            }
            let object = h1
                .find(Group::new(b"mach"), model)
                .or_else(|| h1.find(Group::new(b"scen"), model))
                .unwrap();
            conv.object(object).unwrap();
            let named = |group: &[u8; 4], name: &str| {
                conv.tree
                    .tags
                    .iter()
                    .enumerate()
                    .find(|(_, (n, c))| n == name && c.group == FourCC(*group))
                    .unwrap_or_else(|| panic!("{level}: missing {name}"))
                    .0
            };
            let record = |i: usize| {
                let (_, tag) = &conv.tree.tags[i];
                from_built(Defs::builtin().group(tag.group).unwrap(), &tag.root).unwrap()
            };
            let material = record(named(b"mode", model));
            let i = named(b"shad", shader);
            assert!(
                matches!(material.at(&format!("materials[{slot}].shader")),
                Some(Val::Ref { target: Target::Id(id), .. }) if *id == local(i)),
                "{level} {model} slot {slot}"
            );
            let instance = record(i);
            assert_eq!(
                instance.at("postprocess[0].bitmaps[2].bitmap"),
                Some(&Val::Datum(Target::Id(white)))
            );
            if shader.ends_with("obj-shield charge") {
                let mask = named(b"bitm", r"levels\a10\bitmaps\light");
                assert_eq!(
                    instance.at("postprocess[0].bitmaps[5].bitmap"),
                    Some(&Val::Datum(Target::Id(local(mask))))
                );
            }
            let Some(Val::Ref {
                target: Target::Id(stem),
                ..
            }) = instance.at("template")
            else {
                panic!("{shader}: missing template");
            };
            if layers.is_empty() {
                assert_eq!(*stem, opaque);
            } else {
                let graph = record(local_index(*stem).unwrap());
                let Some(Val::Block(lods)) = graph.at("postprocess[0].lods") else {
                    panic!("missing LODs");
                };
                assert!(!lods.is_empty());
                for lod in lods {
                    let Some(Val::Int(first)) = lod.at("layers") else {
                        panic!("missing layers");
                    };
                    let Some(Val::Int(mask)) = lod.at("layer_mask") else {
                        panic!("missing mask");
                    };
                    for &layer in layers {
                        assert_ne!(*mask & (1 << layer), 0);
                        let l = (*first as usize & 0x1ff) + layer;
                        let Some(Val::Int(passes)) =
                            graph.at(&format!("postprocess[0].layers[{l}].passes"))
                        else {
                            panic!("missing pass range");
                        };
                        assert_eq!(passes >> 9, 1);
                        let p = passes & 0x1ff;
                        let pass = named(b"spas", &format!("{shader}_layer_{layer}"));
                        assert!(matches!(
                            graph.at(&format!("postprocess[0].passes[{p}].pass")),
                            Some(Val::Ref { target: Target::Id(id), .. }) if *id == local(pass)
                        ));
                        let pass = record(pass);
                        let vector = if shader.ends_with("obj-light lock")
                            || (shader.ends_with("light signal") && layer == 7)
                        {
                            1 << 3
                        } else if shader.ends_with("light signal") {
                            1 << 4
                        } else {
                            0
                        };
                        assert_eq!(
                            pass.at("postprocess[0].implementations[0].vector_inputs"),
                            Some(&Val::List(vec![Val::Int(vector), Val::Int(0)]))
                        );
                    }
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 15);
}
