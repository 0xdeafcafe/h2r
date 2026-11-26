use crate::{Error, Material, Materials, ParsedName, PieceDef, Shape, TagKey, starter, units};
use std::collections::HashSet;

fn refused(def: &PieceDef) -> Error {
    def.generate().expect_err("the definition is refused")
}

#[test]
fn each_refusal_is_named() {
    let block = |size: [u16; 3]| PieceDef::dev(Shape::Block, size, 0);
    assert_eq!(
        refused(&block([3, 64, 64])),
        Error::SizeRange {
            axis: 'x',
            value: 3
        }
    );
    assert_eq!(
        refused(&block([64, 2049, 64])),
        Error::SizeRange {
            axis: 'y',
            value: 2049
        }
    );
    assert_eq!(
        refused(&block([64, 64, 0])),
        Error::SizeRange {
            axis: 'z',
            value: 0
        }
    );
    assert!(block([units::MIN, units::MAX, 64]).generate().is_ok());

    for (shape, param) in [
        (Shape::Block, 1),
        (Shape::Stairs, 1),
        (Shape::Stairs, 33),
        (Shape::Cylinder, 10),
        (Shape::Cylinder, 0),
        (Shape::Arch, 3),
        (Shape::Arch, 13),
    ] {
        let err = refused(&PieceDef::dev(shape, [128; 3], param));
        let Error::ParamRange {
            shape: s, param: p, ..
        } = err.clone()
        else {
            panic!("{shape} {param}: {err}");
        };
        assert_eq!((s, p), (shape, param));
        assert!(err.to_string().contains(shape.name()), "{err}");
    }
    for (shape, param) in [
        (Shape::Stairs, 2),
        (Shape::Stairs, 32),
        (Shape::Cylinder, 24),
        (Shape::Arch, 4),
        (Shape::Arch, 12),
    ] {
        assert!(
            PieceDef::dev(shape, [128, 64, 128], param)
                .generate()
                .is_ok()
        );
    }

    let mut def = PieceDef::dev(Shape::Block, [64; 3], 0);
    def.materials = Materials::Each([
        Material::Dev,
        Material::Map(TagKey {
            group: "bitm".into(),
            path: "a".into(),
        }),
        Material::Dev,
        Material::Dev,
    ]);
    assert_eq!(
        refused(&def),
        Error::MapNotShader {
            group: "bitm".into()
        }
    );
    def.materials = Materials::All(Material::Map(TagKey::shader("")));
    assert_eq!(refused(&def), Error::EmptyPath);

    assert_eq!(
        refused(&PieceDef::dev(Shape::Arch, [256, 32, 128], 8)),
        Error::ArchTooLow {
            size_x: 256,
            min_z: 129
        }
    );
    assert!(
        PieceDef::dev(Shape::Arch, [256, 32, 129], 8)
            .generate()
            .is_ok()
    );
    assert_eq!(
        refused(&PieceDef::dev(Shape::Boulder, [128; 3], 7)),
        Error::NotGenerated(Shape::Boulder)
    );
}

#[test]
fn the_name_round_trips() {
    let mut defs = super::support::sample_defs();
    defs.push(PieceDef::dev(Shape::TerrainWedge, [90, 80, 70], 65535));
    for def in defs {
        let name = def.name();
        let parsed = ParsedName::parse(&name).unwrap();
        assert_eq!(parsed.shape, def.shape, "{name}");
        assert_eq!(parsed.size, def.size);
        assert_eq!(parsed.param, def.param);
        assert_eq!(parsed.digest, def.digest12());
        assert!(parsed.matches(&def));
        assert!(def.is_named(&name));
        assert_eq!(def.tag_path(), format!("forge\\blockout\\{name}"));
    }
    let def = PieceDef::dev(Shape::FloorPlate, [64, 64, 16], 0);
    assert!(def.name().starts_with("floor_plate_64x64x16_0_"));
    assert_eq!(def.digest12().len(), 12);
}

#[test]
fn a_bad_name_is_refused_by_what_is_wrong() {
    for (name, why) in [
        ("", "no param"),
        ("block_64x64x64_0", "no shape"),
        (
            "block_64x64x64_0_ABCDEF012345",
            "the digest is not 12 lowercase hex digits",
        ),
        ("cube_64x64x64_0_0123456789ab", "unknown shape"),
        ("block_64x64x64_x_0123456789ab", "param is not a number"),
        ("block_64x64_0_0123456789ab", "size is not <x>x<y>x<z>"),
        ("block_64xaax64_0_0123456789ab", "size is not numbers"),
    ] {
        match ParsedName::parse(name) {
            Err(Error::BadName { why: w, .. }) => assert_eq!(w, why, "{name:?}"),
            other => panic!("{name:?}: {other:?}"),
        }
    }
}

#[test]
fn equal_definitions_have_equal_names_and_a_material_changes_the_digest() {
    let a = PieceDef::dev(Shape::Block, [64, 64, 64], 0);
    let b = PieceDef::dev(Shape::Block, [64, 64, 64], 0);
    assert_eq!(a.name(), b.name());
    let mut c = a.clone();
    c.materials = Materials::All(Material::Nodraw);
    assert_ne!(a.name(), c.name());
    assert_eq!(
        a.canonical_json(),
        "{\"materials\":{\"all\":\"dev\"},\"param\":0,\"shape\":\"block\",\"size\":[64,64,64]}"
    );
    let mut d = a.clone();
    d.materials = Materials::Each([
        Material::Map(TagKey::shader("levels\\a\"b")),
        Material::Dev,
        Material::Nodraw,
        Material::Clip,
    ]);
    assert_eq!(
        d.canonical_json(),
        "{\"materials\":{\"each\":[{\"map\":{\"group\":\"shad\",\"path\":\"levels\\\\a\\\"b\"}},\"dev\",\"nodraw\",\"clip\"]},\"param\":0,\"shape\":\"block\",\"size\":[64,64,64]}"
    );
}

#[test]
fn materials_are_chosen_by_role() {
    use crate::Role;
    let each = Materials::Each([
        Material::Clip,
        Material::Dev,
        Material::Nodraw,
        Material::Dev,
    ]);
    assert_eq!(each.get(Role::Floor), &Material::Clip);
    assert_eq!(each.get(Role::Ceiling), &Material::Nodraw);
    assert_eq!(
        Materials::All(Material::Dev).get(Role::Trim),
        &Material::Dev
    );
}

#[test]
fn the_starter_set_is_valid_distinct_and_generates() {
    let defs = starter();
    assert!((20..=30).contains(&defs.len()));
    let names: HashSet<String> = defs.iter().map(PieceDef::name).collect();
    assert_eq!(names.len(), defs.len());
    for def in &defs {
        let solid = def
            .generate()
            .unwrap_or_else(|e| panic!("{}: {e}", def.name()));
        assert!(solid.volume() > 0.0);
    }
}

#[test]
fn the_shape_table_is_consistent() {
    for shape in Shape::ALL {
        assert_eq!(Shape::from_name(shape.name()), Some(shape));
    }
    assert_eq!(Shape::from_name("nope"), None);
    assert!(units::GRIDS.iter().all(|g| g.is_power_of_two()));
    assert!(units::GRIDS.contains(&units::DEFAULT_GRID));
}
