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
        let mut flat = Flat {
            leaves: Vec::new(),
            titles: BTreeMap::new(),
        };
        flatten(d, 0, "", None, "", None, &mut flat);
        for leaf in flat.leaves {
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
fn repeated_embedded_structs_keep_the_description_of_each_use() {
    let mut defs = Defs::default();
    defs.parse_into(
        "invented",
        r#"struct sample 8
  0x0 u32 turns doc="Turns in one cycle." -- invented leaf evidence
  0x4 unknown 4
group zzzz 0x18
  0x0 struct first sample doc="First spindle." -- first source
  0x8 struct second sample doc="Second spindle." -- second source
  0x10 struct third sample
"#,
    )
    .unwrap();
    let registry = Registry::build(&Schema::default(), &defs);
    let layout = registry.group(FourCC(*b"zzzz")).unwrap();
    let docs: Vec<_> = layout.fields.iter().map(|f| f.doc.as_deref()).collect();
    assert_eq!(
        docs,
        [
            Some("First spindle. Turns in one cycle."),
            Some("First spindle."),
            Some("Second spindle. Turns in one cycle."),
            Some("Second spindle."),
            Some("Turns in one cycle."),
            None,
        ]
    );
    assert_eq!(
        layout.field_at(0).unwrap().meaning,
        "first: first source\ninvented leaf evidence"
    );
    assert_eq!(layout.field_at(12).unwrap().meaning, "second: second source");
    assert_eq!(layout.field_at(16).unwrap().meaning, "invented leaf evidence");
}

#[test]
fn unknown_descriptions_cover_whole_fallback_rows() {
    let schema = Schema::parse("group zzzz 0x14 align 4\n  0x8 string\n").unwrap();
    let mut defs = Defs::default();
    defs.parse_into(
        "invented",
        r#"group zzzz 0x14
  0x0 unknown 6 doc="Invented first region."
  0x6 unknown 10 doc="Invented second region." -- invented span evidence
  0x10 unknown 4
"#,
    )
    .unwrap();
    let r = Registry::build(&schema, &defs);
    let layout = r.group(FourCC(*b"zzzz")).unwrap();
    assert_eq!(layout.field_at(8).unwrap().meaning, "invented span evidence");
    assert_eq!(layout.field_at(12).unwrap().meaning, "invented span evidence");
    assert!(layout.field_at(4).unwrap().meaning.is_empty());
    let rows: Vec<_> = layout
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.kind.width(), f.doc.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [
            ("+0x0", 4, Some("Invented first region.")),
            ("+0x4", 4, None),
            ("string +0x8", 4, Some("Invented second region.")),
            ("+0xc", 4, Some("Invented second region.")),
            ("+0x10", 4, None),
        ]
    );
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

/// An invented group using every editor attribute.
const ATTRS: &str = r#"struct shared 0x8 title="Shared kind" applies=use
  0x0 f32 rate unit=world_units_per_second label="Rate (fast)"
  0x4 f32 flat applies=creation
group test 0x4c title="Test thing" applies=frame
  0x0 struct inner shared
  0x8 f32 delay unit=seconds
  0xc f32[3] offset axes=x|y|z unit=world_units
  0x18 f32 turn_angle unit=seconds
  0x1c f32 spin unit=degrees_per_second
  0x20 f32 swing_angle
  0x24 f32[2] aim_range unit=fraction
  0x2c block parts 0x8 align 4 title=name applies=use
    0x0 string name
    0x4 f32 weight applies=creation
  0x34 block bare 0x4 align 4
    0x0 f32 loose
  0x3c struct late shared applies=creation
  0x44 u32 own applies=use
  0x48 u32 plain
"#;

/// An invented group with a view on a colour and a function.
const VIEW_ATTRS: &str = "group test 0x14\n  \
  0x0 f32[3] tint view=hdr_rgb\n  \
  0xc data curve align 4 view=function\n";

#[test]
fn a_definitions_view_reaches_its_registry_field() {
    let mut defs = Defs::default();
    defs.parse_into("views", VIEW_ATTRS).unwrap();
    let r = Registry::build(&Schema::default(), &defs);
    let l = r.group(FourCC(*b"test")).unwrap();
    assert_eq!(l.field_at(0).unwrap().view, Some(FieldView::HdrRgb));
    assert_eq!(l.field_at(0xc).unwrap().view, Some(FieldView::Function));
}

fn attr_layout() -> Arc<Layout> {
    let mut defs = Defs::default();
    defs.parse_into("attrs", ATTRS).unwrap();
    let r = Registry::build(&Schema::default(), &defs);
    assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    r.group(FourCC(*b"test")).unwrap().clone()
}

#[test]
fn definition_attributes_reach_the_fields() {
    let l = attr_layout();
    assert_eq!(l.title, Some(Title::Text("Test thing".into())));
    assert_eq!(l.section_titles["inner"], "Shared kind");
    assert_eq!(l.section_titles["late"], "Shared kind");
    assert_eq!(l.section_titles.len(), 2);
    let rate = l.field_at(0).unwrap();
    assert_eq!(rate.section, "inner");
    assert_eq!(rate.label.as_deref(), Some("Rate (fast)"));
    assert_eq!(rate.unit, Some(Unit::WorldUnitsPerSecond));
    assert_eq!(l.field_at(0xc).unwrap().axes, ["x", "y", "z"]);
    assert_eq!(l.field_at(0xc).unwrap().unit, Some(Unit::WorldUnits));
    assert_eq!(l.field_at(0x8).unwrap().unit, Some(Unit::Seconds));
    let plain = l.field_at(0x48).unwrap();
    assert_eq!(
        (plain.label.clone(), plain.unit, plain.axes.len()),
        (None, None, 0)
    );
    let Kind::Block(parts) = &l.field_at(0x2c).unwrap().kind else {
        panic!("parts is a block")
    };
    assert_eq!(parts.title, Some(Title::Field("name".into())));
    let Kind::Block(bare) = &l.field_at(0x34).unwrap().kind else {
        panic!("bare is a block")
    };
    assert_eq!(bare.title, None);
    assert!(bare.section_titles.is_empty());
}

#[test]
fn applies_is_inherited_and_a_fields_own_value_wins() {
    let l = attr_layout();
    let at = |o: u32| l.field_at(o).unwrap().applies;
    // The group line gives its timing to fields with none of their own.
    assert_eq!(at(0x8), Some("frame"));
    assert_eq!(at(0x48), Some("frame"));
    // A field's own value beats every line above it.
    assert_eq!(at(0x44), Some("use"));
    // The struct's header is nearer its fields than the group line: `rate`
    // takes `use`; `flat` has its own.
    assert_eq!(at(0x0), Some("use"));
    assert_eq!(at(0x4), Some("creation"));
    // The header also beats the line that embeds the struct.
    assert_eq!(at(0x3c), Some("use"));
    assert_eq!(at(0x40), Some("creation"));
    // A block line gives its timing to the element's fields, and a field
    // there keeps its own.
    let Kind::Block(parts) = &l.field_at(0x2c).unwrap().kind else {
        panic!("parts is a block")
    };
    assert_eq!(at(0x2c), Some("use"));
    assert_eq!(parts.field_at(0).unwrap().applies, Some("use"));
    assert_eq!(parts.field_at(4).unwrap().applies, Some("creation"));
    // A block with no timing of its own inherits the group's.
    let Kind::Block(bare) = &l.field_at(0x34).unwrap().kind else {
        panic!("bare is a block")
    };
    assert_eq!(at(0x34), Some("frame"));
    assert_eq!(bare.field_at(0).unwrap().applies, Some("frame"));
}

#[test]
fn a_definition_with_no_applies_anywhere_is_not_known() {
    let mut defs = Defs::default();
    defs.parse_into("t", "group test 0x8\n  0x0 f32 a\n  0x4 u32 b\n")
        .unwrap();
    let r = Registry::build(&Schema::default(), &defs);
    let l = r.group(FourCC(*b"test")).unwrap();
    assert!(l.fields.iter().all(|f| f.applies.is_none()));
}

#[test]
fn a_unit_decides_whether_a_float_shows_as_an_angle() {
    let l = attr_layout();
    let hint = |o: u32| match l.field_at(o).unwrap().kind {
        Kind::Float { hint, .. } => hint,
        _ => panic!("a float"),
    };
    // `degrees_per_second` makes an angle whatever the name.
    assert_eq!(hint(0x1c), Hint::Angle);
    // Another unit overrides the name match.
    assert_eq!(hint(0x18), Hint::Plain);
    // No unit: the name still decides.
    assert_eq!(hint(0x20), Hint::Angle);
    // A unit does not take a bounds name away.
    assert_eq!(hint(0x24), Hint::Bounds);
}

#[test]
fn a_shader_bitmap_slot_applies_on_the_next_frame() {
    // Every field of a postprocess bitmap entry is read at each draw
    // (docs/graphics/hot-iteration.md, "What shows when"); the colour and
    // vector entries carry their own word.
    let r = Registry::builtin();
    let l = r.group(FourCC(*b"shad")).unwrap();
    let Kind::Block(post) = &l.field_at(0x20).unwrap().kind else {
        panic!("postprocess is a block")
    };
    let Kind::Block(bitmaps) = &post.field_at(0x4).unwrap().kind else {
        panic!("bitmaps is a block")
    };
    for at in [0x0, 0x4, 0x8] {
        assert_eq!(bitmaps.field_at(at).unwrap().applies, Some("frame"), "{at:#x}");
    }
    // The parameter block is never read by the draw path: no timing.
    assert_eq!(l.field_at(0xc).unwrap().applies, None);
}
