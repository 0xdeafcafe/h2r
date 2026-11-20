//! The registry against the checked-in schema and definitions, and on an
//! invented group.

use super::*;
use crate::schema::Field as S;

/// Fields ascending, tiling `0..size` exactly, at every depth; and every
/// schema field present at its offset with its type.
fn check(path: &str, l: &Layout, s: Option<&schema::Struct>, bad: &mut Vec<String>) {
    let mut at = 0;
    for f in &l.fields {
        if f.offset != at {
            bad.push(format!(
                "{path}: {} at {:#x}, previous field ends at {at:#x}",
                f.name, f.offset
            ));
        }
        if f.kind.width() == 0 {
            bad.push(format!("{path}: {} is empty", f.name));
        }
        at = f.offset + f.kind.width();
    }
    if at != l.size {
        bad.push(format!(
            "{path}: fields end at {at:#x}, element {:#x}",
            l.size
        ));
    }
    let Some(s) = s else { return };
    if s.size != l.size {
        bad.push(format!(
            "{path}: element {:#x}, schema {:#x}",
            l.size, s.size
        ));
    }
    for (o, sf) in &s.fields {
        let p = format!("{path}+{o:#x}");
        let Some(f) = l.field_at(*o) else {
            bad.push(format!("{p}: no field starts here (schema {sf:?})"));
            continue;
        };
        match (&f.kind, sf) {
            (Kind::Block(child), S::Block(sc)) => check(&p, child, Some(sc), bad),
            (Kind::Data, S::Block(sc)) if is_data(sc) => {}
            (Kind::TagRef { .. }, S::TagRef)
            | (Kind::Datum { .. }, S::TagId)
            | (Kind::StringId, S::StringId)
            | (Kind::Raw, S::Raw)
            | (Kind::Bsp, S::Bsp)
            | (Kind::Word, S::Word) => {}
            (Kind::RawSize { of }, S::RawSize(so)) if of == so => {}
            (k, sf) => bad.push(format!("{p}: registry {}, schema {sf:?}", k.label())),
        }
    }
}

#[test]
fn every_group_s_fields_tile_its_elements_and_hold_the_schema_s_fields() {
    let r = Registry::builtin();
    let schema = Schema::builtin();
    let mut bad = Vec::new();
    for (g, s) in &schema.groups {
        let l = r.group(*g).expect("a layout for every schema group");
        check(&defs::fourcc_text(*g), l, Some(s), &mut bad);
    }
    for (g, l) in &r.groups {
        check(&defs::fourcc_text(*g), l, None, &mut bad);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn the_definitions_merge_without_conflicts() {
    let r = Registry::builtin();
    assert!(r.conflicts.is_empty(), "{}", r.conflicts.join("\n"));
}

/// Every named definition field is in the registry at its offset (embedded
/// structs flattened) with its name, and every layout-dependent one at the
/// schema's offset.
#[test]
fn named_definition_fields_sit_at_their_offsets() {
    fn walk(path: &str, d: &StructDef, l: &Layout, bad: &mut Vec<String>) {
        let mut leaves = Vec::new();
        flatten(d, 0, "", &mut leaves);
        for leaf in leaves {
            let f = leaf.def;
            if f.kind.is_unknown() || f.typed_only() || f.name.starts_with("unknown") {
                continue;
            }
            let Some(rf) = l.field_at(leaf.at) else {
                bad.push(format!("{path}+{:#x} {}: missing", leaf.at, f.name));
                continue;
            };
            if rf.name != f.name || !rf.named || rf.kind.width() != f.kind.width() {
                bad.push(format!(
                    "{path}+{:#x}: {} in the definition, {} ({}) in the registry",
                    leaf.at,
                    f.name,
                    rf.name,
                    rf.kind.label()
                ));
            }
            if let (DefKind::Block(sub), Kind::Block(child)) = (&f.kind, &rf.kind) {
                walk(&format!("{path}.{}", f.name), sub, child, bad);
            }
        }
    }
    let r = Registry::builtin();
    let schema = Schema::builtin();
    let mut bad = Vec::new();
    for (g, d) in &Defs::builtin().groups {
        let l = r.group(*g).unwrap();
        walk(&defs::fourcc_text(*g), d, l, &mut bad);
        // Layout-dependent definition fields at schema offsets (the
        // definitions' own agreement check, through the registry).
        let a = defs::agree(d, &schema.groups[g]).unwrap();
        bad.extend(a.missing);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn coverage_counts_every_byte_once() {
    let c = Registry::builtin().coverage();
    eprintln!(
        "registry: {} named fields, {} unnamed; bytes named {} typed {} pad {} unknown {}",
        c.named_fields, c.unnamed_fields, c.named, c.typed, c.pad, c.unknown
    );
    assert!(c.named_fields > 500, "{c:?}");
    assert!(c.unnamed_fields > 0);
}

#[test]
fn an_invented_group_merges_names_types_and_gaps() {
    let schema = Schema::parse(
        "group test 0x30 align 4\n  0x8 tagref\n  0x10 string\n  0x14 block 0xc align 4\n    0x4 datum\n  0x1c block 0x1 align 16\n  0x28 string\n",
    )
    .unwrap();
    let mut defs = Defs::default();
    defs.parse_into(
        "t",
        "\
group test 0x30
  0x0 i16 kind
  0x2 flags16 flags bits=a|b
  0x4 f32 turn_angle
  0x8 tagref model groups=hlmt
  0x10 string string_10
  0x14 block parts 0xc align 4
    0x0 unknown 0x4
    0x4 datum owner
    0x8 enum16 shape values=box|ball
    0xa unknown 0x2
  0x1c data blob align 16
  0x24 f32 colour_scale
  0x28 unknown 0x8
",
    )
    .unwrap();
    let r = Registry::build(&schema, &defs);
    assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    let l = r.group(FourCC(*b"test")).unwrap();
    let mut bad = Vec::new();
    check("test", l, schema.groups.values().next(), &mut bad);
    assert!(bad.is_empty(), "{bad:?}");
    let names: Vec<(u32, &str, bool)> = l
        .fields
        .iter()
        .map(|f| (f.offset, f.name.as_str(), f.named))
        .collect();
    assert_eq!(
        names,
        [
            (0x0, "kind", true),
            (0x2, "flags", true),
            (0x4, "turn_angle", true),
            (0x8, "model", true),
            (0x10, "string +0x10", false),
            (0x14, "parts", true),
            (0x1c, "blob", true),
            (0x24, "colour_scale", true),
            // The schema's string ID inside the definition's unknown span,
            // then the rest of it as an unnamed word.
            (0x28, "string +0x28", false),
            (0x2c, "+0x2c", false),
        ]
    );
    assert_eq!(
        l.field_at(4).unwrap().kind,
        Kind::Float {
            count: 1,
            hint: Hint::Angle
        }
    );
    let Kind::Block(parts) = &l.field_at(0x14).unwrap().kind else {
        panic!("parts is a block")
    };
    let p: Vec<(u32, String)> = parts
        .fields
        .iter()
        .map(|f| (f.offset, f.kind.label()))
        .collect();
    assert_eq!(
        p,
        [
            (0, "? 4".to_owned()),
            (4, "datum".into()),
            (8, "enum16".into()),
            (0xa, "? 2".into())
        ]
    );
    // A blank element nulls its datum.
    assert_eq!(parts.blank()[4..8], [0xff; 4]);
}

#[test]
fn a_schema_only_group_is_all_typed_or_unknown() {
    let schema =
        Schema::parse("group only 0x13 align 4\n  0x0 tagref\n  0x8 block 0x1 align 4\n").unwrap();
    let r = Registry::build(&schema, &Defs::default());
    let l = r.group(FourCC(*b"only")).unwrap();
    let kinds: Vec<(u32, String)> = l
        .fields
        .iter()
        .map(|f| (f.offset, f.kind.label()))
        .collect();
    assert_eq!(
        kinds,
        [
            (0, "tagref".to_owned()),
            (8, "data".into()),
            (0x10, "? 2".into()),
            (0x12, "? 1".into())
        ]
    );
    assert!(l.fields.iter().all(|f| !f.named));
}
