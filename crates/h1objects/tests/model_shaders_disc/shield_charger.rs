use super::{Base, Converter, Group, h1_map, h2_map, local};
use h1map::{
    Block, TagRef,
    bitmap::{Bitmap, PixelFormat},
};
use map::FourCC;
use tags::defs::{Defs, Record, Target, Val, from_built};
use tags::effects::function::Function;

const DEVICE: &str = r"levels\a10\devices\shield charge\shield charge";
const LAMP: &str = r"levels\a10\shaders\obj-shield charge";
const MASK: &str = r"levels\a10\bitmaps\light";
const FIELD: &str = r"levels\a10\devices\shield charge\shaders\shield charge force";
const IMAGE: &str = r"levels\a10\devices\shield charge\bitmaps\shield charge force";

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn float(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn curve(record: &Record, path: &str) -> Function {
    let Some(Val::Data(blob)) = record.at(path) else {
        panic!("missing curve at {path}")
    };
    Function::parse(blob).unwrap()
}

fn value(f: &Function, x: f32) -> f32 {
    f.range[0] + f.evaluate(x, 0.0) * (f.range[1] - f.range[0])
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn a10_shield_charger_keeps_its_lamp_four_scrolls_and_power_colours() {
    let h1 = h1_map("a10");
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    let mut conv = Converter::new(&h1, &base);
    let source = |group, name: &str| h1.find(Group::new(group), name).unwrap();
    let machine = source(b"mach", DEVICE);
    conv.object(machine).unwrap();
    let named = |group: &[u8; 4], name: &str| {
        conv.tree
            .tags
            .iter()
            .position(|(n, c)| n == name && c.group == FourCC(*group))
            .unwrap_or_else(|| panic!("{name}: missing {}", FourCC(*group)))
    };
    let record = |i: usize| {
        let c = &conv.tree.tags[i].1;
        from_built(Defs::builtin().group(c.group).unwrap(), &c.root).unwrap()
    };
    let link = |rec: &Record, path: &str, index| match rec.at(path) {
        Some(
            Val::Ref {
                target: Target::Id(id),
                ..
            }
            | Val::Datum(Target::Id(id)),
        ) => assert_eq!(*id, local(index), "{path}"),
        other => panic!("{path}: {other:?}"),
    };
    let mode = record(named(b"mode", DEVICE));
    for (slot, shader, image, bitmap_slot, size, format) in [
        (4, LAMP, MASK, 5, 16, PixelFormat::Dxt1),
        (5, FIELD, IMAGE, 0, 128, PixelFormat::Dxt3),
    ] {
        let shader_id = named(b"shad", shader);
        link(&mode, &format!("materials[{slot}].shader"), shader_id);
        let bitmap_id = named(b"bitm", image);
        link(
            &record(shader_id),
            &format!("postprocess[0].bitmaps[{bitmap_slot}].bitmap"),
            bitmap_id,
        );
        let bitmap = Bitmap::read(&h1, source(b"bitm", image)).unwrap();
        let img = &bitmap.images[0];
        assert_eq!((img.width, img.height, img.format), (size, size, format));
        let payload = h1bitmaps::decode::layout_len(img).unwrap();
        assert_eq!(
            &conv.tree.tags[bitmap_id].1.raw[0].0[..payload],
            &bitmap.pixels(&h1, 0).unwrap()[..payload]
        );
    }

    let soso = h1.tag_data(source(b"soso", LAMP)).unwrap();
    assert_eq!(
        TagRef::read(soso, 0xbc).unwrap().id,
        source(b"bitm", MASK).id
    );
    assert_eq!([0x4c, 0x70, 0x72].map(|at| word(soso, at)), [0; 3]);
    assert_eq!(
        [0x84, 0x88, 0x8c].map(|at| float(soso, at)),
        [1.0, 1.0, f32::from_bits(0x3f37b7b7)]
    );
    assert_eq!((float(soso, 0x9c), float(soso, 0xa0)), (1.0, 1.0));
    for (at, amplitude) in [(0xfc, 1.0), (0x10c, 1.0), (0x11c, 360.0)] {
        assert_eq!((word(soso, at), word(soso, at + 2)), (0, 0));
        assert_eq!(
            [4, 8, 12].map(|k| float(soso, at + k)),
            [1.0, 0.0, amplitude]
        );
    }
    let lamp = record(named(b"shad", LAMP));
    assert_eq!(
        lamp.at("postprocess[0].vectors[1].vector"),
        Some(&Val::floats(&[1.0, 1.0, 0.0, 0.0]))
    );
    assert!(
        matches!(lamp.at("postprocess[0].properties"), Some(Val::Block(rows)) if rows.is_empty())
    );
    let lamp_stem = record(named(b"stem", LAMP));
    let Some(Val::Block(bindings)) = lamp_stem.at("postprocess[0].bindings") else {
        panic!("lamp bindings")
    };
    for (binding, (target, source)) in
        bindings[bindings.len() - 3..]
            .iter()
            .zip([(0, 5), (0x1b0, 1), (0x1b1, 1)])
    {
        assert_eq!(binding.at("target"), Some(&Val::Int(target)));
        assert_eq!(binding.at("source"), Some(&Val::Int(source)));
    }
    let emission = record(named(b"spas", &format!("{LAMP}_layer_7")));
    assert_eq!(
        emission.at("postprocess[0].implementations[0].vector_inputs"),
        Some(&Val::List(vec![Val::Int(0); 2]))
    );
    let Some(Val::List(pixel)) = emission.at("postprocess[0].implementations[0].pixel_shader")
    else {
        panic!("lamp pixel shader")
    };
    let packed = |offset: usize| {
        u32::from_le_bytes(std::array::from_fn(|i| match pixel[offset + i] {
            Val::Int(v) => v as u8,
            _ => panic!("pixel byte"),
        }))
    };
    assert_eq!(packed(10 * 4), 0x0000ff00); // multipurpose green
    assert_eq!(packed(11 * 4), 0x00ffffb7); // constant upper endpoint

    let sotr = h1.tag_data(source(b"sotr", FIELD)).unwrap();
    let maps = h1.block(Block::read(sotr, 0x54).unwrap(), 0x64).unwrap();
    assert_eq!(maps.len(), 4 * 0x64);
    assert_eq!(word(sotr, 0x2c), 3);
    let stages = h1.block(Block::read(sotr, 0x60).unwrap(), 0x70).unwrap();
    assert_eq!(stages.len(), 4 * 0x70);
    for (k, stage) in stages.chunks_exact(0x70).enumerate() {
        assert_eq!(
            [0x3c, 0x40, 0x44, 0x48].map(|at| word(stage, at)),
            [
                [5, 17, 6, 18],
                [5, 6, 11, 1],
                [13, 11, 0, 0],
                [13, 11, 0, 0]
            ][k]
        );
        assert_eq!(word(stage, 4), [0, 0, 1, 2][k]);
    }
    let field = record(named(b"shad", FIELD));
    let stem = named(b"stem", FIELD);
    let pass = named(b"spas", FIELD);
    link(&field, "template", stem);
    link(&record(stem), "postprocess[0].passes[0].pass", pass);
    let pass = record(pass);
    let Some(Val::Block(states)) = pass.at("postprocess[0].render_states") else {
        panic!("render states")
    };
    for (state, value) in [
        (59, 1),
        (60, 0),
        (62, 1),
        (63, 1),
        (64, 0),
        (67, 0x010101),
        (147, 0),
    ] {
        let row = states
            .iter()
            .find(|r| r.at("state") == Some(&Val::Int(state)))
            .unwrap();
        assert_eq!(row.at("value"), Some(&Val::Int(value)));
    }
    let vertex = named(b"vrtx", FIELD);
    link(
        &pass,
        "postprocess[0].implementations[0].vertex_shader",
        vertex,
    );
    assert!(
        matches!(record(vertex).at("programs"),Some(Val::Block(rows)) if rows.len()==7 && rows.iter().all(|r| matches!(r.at("compiled_shader"),Some(Val::Data(b)) if b.starts_with(&[0x78,0x20]))))
    );
    let periods = [[0.0f32, 3.0], [0.0, -4.78], [-0.5, 3.0], [-1.0, -4.78]];
    for (stage, map) in maps.chunks_exact(0x64).enumerate() {
        assert_eq!(
            TagRef::read(map, 0x1c).unwrap().id,
            source(b"bitm", IMAGE).id
        );
        for (key, n) in [("slot", 0), ("stage", stage as i64), ("source", 0)] {
            assert_eq!(
                pass.at(&format!("postprocess[0].texture_bindings[{stage}].{key}")),
                Some(&Val::Int(n))
            );
        }
        for axis in 0..2 {
            let k = stage * 2 + axis;
            let at = 0x2c + 16 * axis;
            let period = periods[stage][axis];
            let phase = axis as f32;
            assert_eq!(word(map, at + 2), if period == 0.0 { 0 } else { 6 });
            assert_eq!(float(map, at + 4), if period == 0.0 { 1.0 } else { period });
            assert_eq!((float(map, at + 8), float(map, at + 12)), (phase, 1.0));
            assert_eq!(
                field.at(&format!("postprocess[0].animations[{k}].target")),
                Some(&Val::Int(0x70 + k as i64))
            );
            assert_eq!(
                field.at(&format!("postprocess[0].animations[{k}].property_range")),
                Some(&Val::Int(k as i64))
            );
            let prop = format!("postprocess[0].properties[{k}]");
            let f = curve(&field, &format!("{prop}.function"));
            let Some(Val::Float(duration)) = field.at(&format!("{prop}.input_b")) else {
                panic!("period")
            };
            for t in [0.0f32, 0.6, 2.5, 5.3] {
                let want = if period == 0.0 {
                    0.0
                } else {
                    (t + phase) / period
                };
                let got = value(&f, (t / duration).rem_euclid(1.0));
                let diff = (got - want).rem_euclid(1.0);
                assert!(
                    diff.min(1.0 - diff) < 1e-5,
                    "stage {stage} axis {axis} time {t}: {got} vs {want}"
                );
            }
        }
    }
    for (i, bytes) in [[0x80, 0, 4, 3], [0xc0, 0, 4, 4]].into_iter().enumerate() {
        assert_eq!(
            pass.at(&format!("postprocess[0].engine_bindings[{i}].bytes")),
            Some(&Val::List(bytes.map(Val::Int).to_vec()))
        );
    }
    let h1_device = h1.tag_data(machine).unwrap();
    let colours = h1
        .block(Block::read(h1_device, 0x164).unwrap(), 0x2c)
        .unwrap();
    let functions = h1objects::h1::functions(&h1, h1_device).unwrap();
    assert_eq!((word(h1_device, 0x198), word(h1_device, 0x19c)), (1, 1));
    let mach = record(named(b"mach", DEVICE));
    for (k, name) in ["a_out", "b_out", "c_out"].into_iter().enumerate() {
        let src = &functions[k];
        assert_eq!(
            (
                src.scale_function_by,
                src.square_threshold,
                src.result_scale
            ),
            [(1, 0.0, 10.0), (1, 0.5, 10.0), (3, 0.0, 0.0)][k]
        );
        let path = format!("devi.obje.functions[{k}]");
        assert_eq!(
            mach.at(&format!("{path}.string_4")),
            Some(&Val::text("power"))
        );
        assert_eq!(mach.at(&format!("{path}.output")), Some(&Val::text(name)));
        let f = curve(&mach, &format!("{path}.data_14"));
        for power in [0.0f32, 0.05, 0.1, 0.49, 0.5, 0.5f32.next_up(), 0.75, 1.0] {
            let want = [
                (10.0 * power).min(1.0),
                if power > 0.5 { 1.0 } else { 0.0 },
                power,
            ][k];
            assert_eq!(value(&f, power), want, "{name} at {power}");
        }
        if k < 2 {
            let src = &colours[0x2c * k..];
            assert_eq!((word(src, 0), word(src, 2)), (0, 5 + k as u16));
            let warm = f32::from_bits(0x3ee2e2da);
            assert_eq!(
                [8, 12, 16, 20, 24, 28].map(|at| float(src, at)),
                [
                    [0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                    [1.0, 1.0, 1.0, 1.0, warm, warm]
                ][k]
            );
            let path = format!("devi.obje.change_colors[{k}].block_8[0]");
            assert_eq!(
                mach.at(&format!("{path}.string_24")),
                Some(&Val::text(name))
            );
            for at in (8..0x20).step_by(4) {
                assert_eq!(
                    mach.at(&format!("{path}.unknown_{at:x}")),
                    Some(&Val::Bytes(src[at..at + 4].to_vec()))
                );
            }
        }
    }
}
