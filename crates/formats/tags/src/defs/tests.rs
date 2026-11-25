//! Definitions on invented tags, and against the checked-in schema.

use super::*;
use crate::repo::{Body, LooseTag, Node, Region, TagKey, Tree, Value};
use crate::schema::Schema;
use map::TagId;

const TEXT: &str = r#"struct base 0x8
  0x0 i16 kind doc="Selects \"plain\" -- the default \\ preset." default=6 applies=use -- E: test
  0x2 flags16 flags bits=a|b|_|c default=b|c
  0x4 f32 radius default=1.5
group test 0x34
  0x0 struct base base
  0x8 tagref model groups=hlmt
  0x10 string name
  0x14 block parts 0xc align 4
    0x0 string name
    0x4 datum owner
    0x8 unknown 0x2 seen=zero
    0xa pad 0x2
  0x1c data blob align 16
  0x24 enum16 shape values=box|sphere default=sphere
  0x26 unknown 0x2
  0x28 f32[3] offset
"#;

fn defs() -> Defs {
    let mut d = Defs::default();
    d.parse_into("test", TEXT).unwrap();
    d
}

#[test]
fn field_descriptions_keep_quoted_text_separate_from_evidence() {
    let defs = defs();
    let kind = defs.structs["base"].field("kind").unwrap();
    assert_eq!(
        kind.doc.as_deref(),
        Some(r#"Selects "plain" -- the default \ preset."#)
    );
    assert_eq!(kind.meaning, "E: test");
    assert_eq!(kind.default.as_deref(), Some("6"));
    assert_eq!(kind.applies, Some("use"));
    assert!(defs.structs["base"].field("flags").unwrap().doc.is_none());
    let registry = crate::registry::Registry::build(&Schema::default(), &defs);
    let layout = registry.group(FourCC(*b"test")).unwrap();
    assert_eq!(layout.field_at(0).unwrap().doc, kind.doc);
}

#[test]
fn malformed_field_descriptions_report_the_line() {
    for attr in [
        "doc=bare",
        "doc=\"unfinished",
        "doc=\"one\"tail",
        "doc=\"one\" doc=\"two\"",
    ] {
        let text = format!("group zzzz 4\n  0x0 u32 invented {attr}");
        let error = Defs::default().parse_into("invented", &text).unwrap_err();
        assert_eq!(error.line, 2);
    }
}

#[test]
fn live_apply_classes_require_known_names_and_preserve_the_evidence() {
    for which in ["frame", "use", "creation"] {
        let mut defs = Defs::default();
        let text = format!(
            "group test 0x8\n  0x0 f32 rate applies={which} -- invented reader\n  0x4 u32 count\n"
        );
        defs.parse_into("invented", &text).unwrap();
        let group = defs.group(FourCC(*b"test")).unwrap();
        let field = group.field("rate").unwrap();
        assert_eq!(field.applies, Some(which));
        assert_eq!(field.meaning, "invented reader");
        assert_eq!(group.field("count").unwrap().applies, None);
    }
    assert!(
        Defs::default()
            .parse_into("invented", "group test 0x4\n  0x0 f32 rate applies=soon")
            .is_err()
    );
}

fn key(g: &[u8; 4], p: &str) -> TagKey {
    TagKey::new(FourCC(*g), p)
}

fn sample() -> LooseTag {
    let mut root = vec![0u8; 0x34];
    root[0..2].copy_from_slice(&3i16.to_le_bytes());
    root[4..8].copy_from_slice(&2.0f32.to_le_bytes());
    root[8..12].copy_from_slice(&u32::from_be_bytes(*b"hlmt").to_le_bytes());
    root[0x14..0x18].copy_from_slice(&2u32.to_le_bytes());
    root[0x1c..0x20].copy_from_slice(&3u32.to_le_bytes());
    root[0x26] = 0x7f;
    let mut parts = vec![0u8; 0x18];
    parts[0x8] = 1;
    parts[0xc + 4..0xc + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    LooseTag {
        key: key(b"test", "a\\b"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![
                Node {
                    bytes: root,
                    count: 1,
                    size: 0x34,
                    align: 4,
                    fields: vec![
                        (0xc, Value::Ref(key(b"hlmt", "m"))),
                        (0x10, Value::String("thing".into())),
                        (0x14, Value::Block(1)),
                        (0x1c, Value::Data(2)),
                    ],
                },
                Node {
                    bytes: parts,
                    count: 2,
                    size: 0xc,
                    align: 4,
                    fields: vec![
                        (0x0, Value::String("p".into())),
                        (0x4, Value::Ref(key(b"mode", "x"))),
                    ],
                },
                Node {
                    bytes: vec![1, 2, 3],
                    count: 3,
                    size: 1,
                    align: 16,
                    fields: vec![],
                },
            ],
            raws: vec![],
            losses: vec![],
        }),
    }
}

#[test]
fn a_tag_decodes_and_encodes_back_exactly() {
    let d = defs();
    let def = d.group(FourCC(*b"test")).unwrap();
    let tag = sample();
    let rec = decode(def, &tag).unwrap();
    assert_eq!(rec.at("base.kind"), Some(&Val::Int(3)));
    assert_eq!(
        rec.at("model"),
        Some(&Val::Ref {
            group: u32::from_be_bytes(*b"hlmt"),
            target: Target::Key(key(b"hlmt", "m")),
        })
    );
    assert_eq!(rec.at("parts[0].name"), Some(&Val::text("p")));
    assert_eq!(rec.at("parts[1].owner"), Some(&Val::Datum(Target::Null)));
    assert_eq!(rec.at("parts[0].unknown_8"), Some(&Val::Bytes(vec![1, 0])));
    assert_eq!(rec.at("blob"), Some(&Val::Data(vec![1, 2, 3])));
    let back = encode(def, tag.key.clone(), &rec, Region::Metadata).unwrap();
    assert_eq!(back, tag);
    assert_eq!(canonical(&back), canonical(&tag));
}

#[test]
fn a_fixup_inside_a_plain_field_is_refused() {
    let d = defs();
    let def = d.group(FourCC(*b"test")).unwrap();
    let mut tag = sample();
    let Body::Tree(t) = &mut tag.body else {
        unreachable!()
    };
    t.nodes[0].fields.push((0x28, Value::String("x".into())));
    let e = decode(def, &tag).unwrap_err();
    assert_eq!(e.path, "offset");
}

#[test]
fn defaults_fill_every_field() {
    let d = defs();
    let def = d.group(FourCC(*b"test")).unwrap();
    let r = default_record(def);
    assert_eq!(r.at("base.kind"), Some(&Val::Int(6)));
    assert_eq!(r.at("base.flags"), Some(&Val::Flags(0b1010)));
    assert_eq!(r.at("base.radius"), Some(&Val::Float(1.5)));
    assert_eq!(r.at("shape"), Some(&Val::Enum(1)));
    assert_eq!(r.at("unknown_26"), Some(&Val::Bytes(vec![0, 0])));
    let t = encode(def, key(b"test", "fresh"), &r, Region::Metadata).unwrap();
    assert_eq!(decode(def, &t).unwrap(), r);
}

#[test]
fn shared_arrays_encode_once() {
    let mut d = Defs::default();
    d.parse_into(
        "t",
        "group twob 0x10\n  0x0 block a 0x4\n    0x0 u32 v\n  0x8 block b 0x4\n    0x0 u32 v\n",
    )
    .unwrap();
    let def = d.group(FourCC(*b"twob")).unwrap();
    let mut r = default_record(def);
    let elem = Record::new(vec![("v".into(), Val::Int(7))]);
    r.set("a", Val::Block(vec![elem.clone()])).unwrap();
    r.set("b", Val::Block(vec![elem])).unwrap();
    let t = encode(def, key(b"twob", "x"), &r, Region::Metadata).unwrap();
    assert_eq!(t.tree().unwrap().nodes.len(), 2);
}

#[test]
fn diff_lists_fields_by_path() {
    let d = defs();
    let def = d.group(FourCC(*b"test")).unwrap();
    let a = decode(def, &sample()).unwrap();
    let mut b = a.clone();
    b.set("base.radius", Val::Float(4.0)).unwrap();
    b.set("parts[1].name", Val::text("q")).unwrap();
    let c = diff(def, &a, &b);
    let paths: Vec<_> = c.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["base.radius", "parts[1].name"]);
    assert!(b.set("base.radius", Val::Int(1)).is_err());
}

#[test]
fn built_trees_round_trip() {
    let d = defs();
    let def = d.group(FourCC(*b"test")).unwrap();
    let mut r = decode(def, &sample()).unwrap();
    r.set("model", Val::tag(FourCC(*b"hlmt"), TagId(0xe0010002)))
        .unwrap();
    let mut owner = |k: &TagKey| (k.path == "x").then_some(TagId(0xe0020003));
    let b = to_built(def, &r, &mut owner).unwrap();
    let back = from_built(def, &b).unwrap();
    // The builder writes a reference's group word from its target.
    assert_eq!(
        back.at("model"),
        Some(&Val::Ref {
            group: 0,
            target: Target::Id(TagId(0xe0010002)),
        })
    );
    assert_eq!(
        back.at("parts[0].owner"),
        Some(&Val::Datum(Target::Id(TagId(0xe0020003))))
    );
    assert_eq!(back.at("parts[0].name"), Some(&Val::text("p")));
    assert_eq!(back.at("blob"), Some(&Val::Data(vec![1, 2, 3])));
    assert_eq!(back.at("unknown_26"), r.at("unknown_26"));
}

#[test]
fn gaps_and_overlaps_are_refused() {
    let mut d = Defs::default();
    let e = d
        .parse_into("t", "group gapp 0x8\n  0x0 u32 a\n  0x6 u16 b\n")
        .unwrap_err();
    assert!(e.why.contains("every byte"), "{e}");
    let e = d
        .parse_into("t", "group shrt 0x8\n  0x0 u32 a\n")
        .unwrap_err();
    assert!(e.why.contains("fields end"), "{e}");
}

/// The file text of every struct and then group `d` holds.
fn text_of(d: &Defs, group: FourCC) -> String {
    let mut text = String::new();
    for (name, st) in &d.structs {
        text.push_str(&format!(
            "struct {name} {:#x} align {}{}\n",
            st.size,
            st.align,
            st.header_attrs()
        ));
        st.fields_text(2, &mut text);
    }
    let s = &d.groups[&group];
    text.push_str(&format!(
        "group {} {:#x} align {}{}\n",
        fourcc_text(group),
        s.size,
        s.align,
        s.header_attrs()
    ));
    s.fields_text(2, &mut text);
    text
}

#[test]
fn the_text_form_round_trips() {
    let d = defs();
    for (g, s) in &d.groups {
        let text = text_of(&d, *g);
        let mut again = Defs::default();
        again.parse_into("again", &text).unwrap();
        assert_eq!(again.group(*g), Some(s));
    }
}

/// An invented group using every editor attribute.
const ATTRS: &str = r#"struct shared 0x8 title="Shared kind" applies=use
  0x0 f32 rate unit=world_units_per_second label="Rate (fast)"
  0x4 f32 flat applies=creation
group test 0x44 title="Test thing" applies=frame
  0x0 struct inner shared
  0x8 f32 delay unit=seconds label="Wait \"time\""
  0xc f32[3] offset axes=x|y|z unit=world_units
  0x18 i16 level unit=index
  0x1a i16 spare
  0x1c u32 percent_full unit=percent
  0x20 f32[2] span axes=lo|hi
  0x28 f32 turn unit=degrees_per_second
  0x2c block parts 0x8 align 4 title=name applies=use
    0x0 string name
    0x4 f32 weight
  0x34 block bare 0x4 align 4
    0x0 f32 loose
  0x3c struct late shared applies=creation
"#;

fn attr_defs() -> Defs {
    let mut d = Defs::default();
    d.parse_into("attrs", ATTRS).unwrap();
    d
}

#[test]
fn editor_attributes_parse_into_the_model() {
    let d = attr_defs();
    let shared = &d.structs["shared"];
    assert_eq!(shared.title, Some(Title::Text("Shared kind".into())));
    assert_eq!(shared.applies, Some("use"));
    let rate = shared.field("rate").unwrap();
    assert_eq!(rate.unit, Some(Unit::WorldUnitsPerSecond));
    assert_eq!(rate.label.as_deref(), Some("Rate (fast)"));
    assert_eq!(rate.applies, None);
    let g = d.group(FourCC(*b"test")).unwrap();
    assert_eq!(g.title, Some(Title::Text("Test thing".into())));
    assert_eq!(g.applies, Some("frame"));
    let delay = g.field("delay").unwrap();
    assert_eq!(delay.unit, Some(Unit::Seconds));
    assert_eq!(delay.label.as_deref(), Some("Wait \"time\""));
    let offset = g.field("offset").unwrap();
    assert_eq!(offset.axes, ["x", "y", "z"]);
    assert_eq!(offset.unit, Some(Unit::WorldUnits));
    assert_eq!(g.field("span").unwrap().axes, ["lo", "hi"]);
    assert_eq!(g.field("spare").unwrap().unit, None);
    assert!(g.field("spare").unwrap().axes.is_empty());
    let Kind::Block(parts) = &g.field("parts").unwrap().kind else {
        panic!("parts is a block")
    };
    assert_eq!(parts.title, Some(Title::Field("name".into())));
    assert_eq!(g.field("parts").unwrap().applies, Some("use"));
    let Kind::Block(bare) = &g.field("bare").unwrap().kind else {
        panic!("bare is a block")
    };
    assert_eq!(bare.title, None);
    assert_eq!(g.field("late").unwrap().applies, Some("creation"));
}

#[test]
fn every_unit_word_parses_and_prints_back() {
    for unit in Unit::ALL {
        let text = format!("group test 0x4\n  0x0 f32 rate unit={}\n", unit.word());
        let mut d = Defs::default();
        d.parse_into("unit", &text).unwrap();
        let rate = d.group(FourCC(*b"test")).unwrap().field("rate").unwrap();
        assert_eq!(rate.unit, Some(unit));
        assert_eq!(Unit::from_word(unit.word()), Some(unit));
    }
    assert_eq!(Unit::ALL.len(), 11);
    assert_eq!(Unit::from_word("furlongs"), None);
    assert!(Unit::Degrees.is_angle() && Unit::DegreesPerSecond.is_angle());
    assert!(!Unit::Seconds.is_angle());
}

#[test]
fn editor_attributes_round_trip_through_the_text_form() {
    let d = attr_defs();
    let g = FourCC(*b"test");
    let text = text_of(&d, g);
    let mut again = Defs::default();
    again.parse_into("again", &text).unwrap();
    assert_eq!(again, d);
    // Printing what was parsed is stable, byte for byte.
    assert_eq!(text_of(&again, g), text);
    assert!(text.contains("unit=world_units_per_second"));
    assert!(text.contains("axes=x|y|z"));
    assert!(text.contains("title=name"));
    assert!(text.contains("label=\"Wait \\\"time\\\"\""));
}

#[test]
fn wrong_editor_attributes_are_refused_with_the_line() {
    let cases: &[(&str, &str)] = &[
        ("0x0 f32 a unit=furlongs", "unknown unit"),
        ("0x0 enum8 a values=x|y unit=seconds", "numeric"),
        ("0x0 f32 a axes=x|y", "array"),
        ("0x0 f32[3] a axes=x|y", "2 names for 3"),
        ("0x0 f32[2] a axes=x|", "each axis is a word"),
        ("0x0 f32[2] a axes=x|1y", "each axis is a word"),
        ("0x0 f32 a label=plain", "expected label=\""),
        ("0x0 f32 a label=\"\"", "label is empty"),
        ("0x0 f32 a label=\" x\"", "label has a space"),
        ("0x0 f32 a label=\"x  y\"", "label has a double space"),
        ("0x0 f32 a label=\"x\" label=\"y\"", "duplicate"),
        ("0x0 f32 a title=\"Thing\"", "title is for"),
        ("0x0 f32 a title=other", "title is for"),
        ("0x0 unknown 0x4 label=\"x\"", "takes no label"),
        ("0x0 f32 a applies=soon", "unknown applies"),
        (
            "0x0 block a 0x4 align 4 title=nothing\n    0x0 f32 b",
            "no such field",
        ),
        (
            "0x0 block a 0x4 align 4 title=\"Words\"\n    0x0 f32 b",
            "field name",
        ),
        (
            "0x0 block a 0x4 align 4 title=pad_0\n    0x0 pad 0x4",
            "needs a field with a value",
        ),
    ];
    for (line, why) in cases {
        let text = format!("group test 0x8\n  {line}\n  0x4 u32 tail");
        let error = Defs::default()
            .parse_into("invented", &text)
            .expect_err(line);
        assert!(error.why.contains(why), "{line}: {error}");
        assert_eq!(error.line, 2, "{line}");
    }
    for header in [
        "group test 0x4 title=Bare",
        "group test 0x4 title=\"\"",
        "group test 0x4 applies=soon",
        "group test 0x4 colour=red",
        "group test 0x4 title=\"A\" title=\"B\"",
        "group test 0x4 label=\"A\"",
    ] {
        let text = format!("{header}\n  0x0 u32 a");
        let error = Defs::default().parse_into("invented", &text).unwrap_err();
        assert_eq!(error.line, 1, "{header}");
    }
}

#[test]
fn every_builtin_field_description_is_nonempty_and_single_line() {
    fn check(def: &StructDef) {
        for field in &def.fields {
            if let Some(doc) = &field.doc {
                assert!(
                    !doc.is_empty() && doc.trim_end() == doc.as_str() && !doc.contains(['\n', '\r']),
                    "{}.{} needs a nonempty, single-line description without trailing whitespace",
                    def.name,
                    field.name
                );
            }
            if let Kind::Block(child) | Kind::Struct(child) = &field.kind {
                check(child);
            }
        }
    }

    let defs = Defs::builtin();
    for def in defs.groups.values().chain(defs.structs.values()) {
        check(def);
    }
}

#[test]
fn builtin_definitions_agree_with_the_schema() {
    let defs = Defs::builtin();
    let schema = Schema::builtin();
    let mut bad = Vec::new();
    for (g, def) in &defs.groups {
        let s = schema.groups.get(g).expect("schema has the group");
        let a = agree(def, s).unwrap();
        bad.extend(a.missing);
        // A definition-only field is fine (always null in retail); the
        // disc test checks that it is.
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn builtin_definitions_round_trip_their_defaults() {
    for (g, def) in &Defs::builtin().groups {
        let r = default_record(def);
        let t = encode(def, TagKey::new(*g, "fresh"), &r, Region::Metadata).unwrap();
        assert_eq!(decode(def, &t).unwrap(), r, "{g}");
    }
}

#[test]
fn raw_bytes_fill_typed_fields_and_read_back() {
    let mut defs = Defs::default();
    defs.parse_into(
        "invented",
        "group rawb 0x1c
  0x0 i16 low
  0x2 enum16 mode values=a|b
  0x4 f32[2] range
  0xc unknown 0x4
  0x10 string label
  0x14 block parts 0x8 align 4
    0x0 f32 weight
    0x4 u16 first
    0x6 u16 second
",
    )
    .unwrap();
    let d = defs.group(FourCC(*b"rawb")).unwrap();
    let mut r = default_record(d);
    // One word over two fields, each read back as its own type.
    r.set("low", Val::Bytes(vec![1, 0, 2, 0])).unwrap();
    assert_eq!(r.get("low"), Some(&Val::Int(1)));
    assert_eq!(r.get("mode"), Some(&Val::Enum(2)));
    // A name and an offset into it, then an element offset.
    r.set("range+0x4", Val::Bytes(1.5f32.to_le_bytes().to_vec())).unwrap();
    assert_eq!(r.get("range"), Some(&Val::floats(&[0.0, 1.5])));
    r.set("@0xc", Val::Bytes(vec![9; 4])).unwrap();
    assert_eq!(r.get("unknown_c"), Some(&Val::Bytes(vec![9; 4])));
    // An unknown span by its own name is a plain write, as before.
    r.set("unknown_c", Val::Bytes(vec![7; 4])).unwrap();
    assert_eq!(r.bytes("@0xc", 4), Some(vec![7; 4]));
    // Float bits survive, NaN payload included.
    let nan = 0x7fc0_1234u32.to_le_bytes().to_vec();
    r.set("range", Val::Bytes(nan.clone())).unwrap();
    assert_eq!(r.bytes("range", 4), Some(nan));
    // Through a block element.
    let parts = d.child("parts").unwrap();
    r.set("parts", Val::Block(vec![default_record(parts)])).unwrap();
    r.set("parts[0].first", Val::Bytes(vec![3, 0, 4, 0])).unwrap();
    assert_eq!(r.at("parts[0].second"), Some(&Val::Int(4)));
    assert_eq!(r.bytes("parts[0].@0x4", 4), Some(vec![3, 0, 4, 0]));
    // Over a string, or past the element, is refused and changes nothing.
    let before = r.clone();
    assert!(r.set("@0xc", Val::Bytes(vec![0; 8])).is_err());
    assert!(r.set("parts[0].@0x6", Val::Bytes(vec![0; 4])).is_err());
    assert!(r.set("range+0x8", Val::Bytes(vec![0; 4])).is_err());
    // Compared as text: the NaN written above is never equal to itself.
    assert_eq!(format!("{r:?}"), format!("{before:?}"));
    assert_eq!(r.bytes("@0x10", 4), None);
    // A record built without a definition has no layout.
    let mut bare = Record::new(vec![("x".into(), Val::Int(0))]);
    assert!(bare.set("x", Val::Bytes(vec![0; 4])).is_err());
}

/// An invented group with one `view=` word of each kind.
const VIEWS: &str = "group test 0x3c\n  \
  0x0 f32[3] tint view=rgb\n  \
  0xc f32[4] glow view=argb\n  \
  0x1c u32 edge view=rgb8\n  \
  0x20 u32 paint view=argb8\n  \
  0x24 u32 fill view=xrgb8\n  \
  0x28 f32[3] light view=hdr_rgb\n  \
  0x34 data curve align 4 view=function\n";

#[test]
fn every_view_word_parses_prints_back_and_reaches_the_field() {
    let mut d = Defs::default();
    d.parse_into("views", VIEWS).unwrap();
    let g = d.group(FourCC(*b"test")).unwrap();
    let views: Vec<_> = g.fields.iter().map(|f| f.view).collect();
    assert_eq!(
        views,
        FieldView::ALL.map(Some).to_vec(),
        "one word of each kind, in the list's order"
    );
    for v in FieldView::ALL {
        assert_eq!(FieldView::from_word(v.word()), Some(v));
    }
    assert_eq!(FieldView::from_word("rgba"), None);
    assert!(FieldView::Rgb.is_colour() && !FieldView::Function.is_colour());
    let text = text_of(&d, FourCC(*b"test"));
    assert!(text.contains("view=hdr_rgb") && text.contains("view=function"));
    let mut again = Defs::default();
    again.parse_into("again", &text).unwrap();
    assert_eq!(again, d);
    assert_eq!(text_of(&again, FourCC(*b"test")), text);
}

#[test]
fn an_unknown_view_word_or_a_wrong_type_fails_with_the_line() {
    let cases: &[(&str, u32, &str)] = &[
        ("0x0 f32[3] a view=purple", 0xc, "unknown view=purple"),
        ("0x0 f32[3] a view=", 0xc, "unknown view="),
        ("0x0 f32[3] a view=argb", 0xc, "f32[4]"),
        ("0x0 f32[4] a view=rgb", 0x10, "f32[3]"),
        ("0x0 f32 a view=rgb", 4, "f32[3]"),
        ("0x0 u16 a view=rgb8", 2, "4-byte integer"),
        ("0x0 u32 a view=function", 4, "data reference"),
        ("0x0 data a align 4 view=rgb", 8, "does not fit"),
        ("0x0 enum8 a values=x|y view=rgb8", 1, "does not fit"),
    ];
    for (line, size, why) in cases {
        let text = format!("group test {size:#x}\n  {line}\n");
        let error = Defs::default()
            .parse_into("invented", &text)
            .expect_err(line);
        assert!(error.why.contains(why), "{line}: {error}");
        assert_eq!(error.line, 2, "{line}");
    }
}

/// Every `applies=` in the checked-in definitions carries the engine or
/// disc evidence it rests on: an `E` or a `V` word in the meaning.
#[test]
fn builtin_applies_lines_carry_evidence() {
    fn walk(path: &str, def: &StructDef, found: &mut u32, bad: &mut Vec<String>) {
        for field in &def.fields {
            let here = format!("{path}.{}", field.name);
            if field.applies.is_some() {
                *found += 1;
                let evidenced = field
                    .meaning
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|w| w == "E" || w == "V");
                if !evidenced {
                    bad.push(here.clone());
                }
            }
            if let Kind::Block(child) | Kind::Struct(child) = &field.kind {
                walk(&here, child, found, bad);
            }
        }
    }
    let defs = Defs::builtin();
    let (mut found, mut bad) = (0, Vec::new());
    for (g, def) in &defs.groups {
        walk(&fourcc_text(*g), def, &mut found, &mut bad);
    }
    assert!(bad.is_empty(), "applies without evidence: {bad:?}");
    assert!(found > 100, "the realtime coverage definitions are present");
}

/// The groups R13 covers hold their timings: HUD widgets and particle
/// draw fields per frame, emitter and effect-part fields when used, and
/// object creation fields at creation.
#[test]
fn builtin_applies_timings_for_the_covered_groups() {
    let defs = Defs::builtin();
    let at = |g: &[u8; 4], path: &str| {
        let mut def = defs.group(FourCC(*g)).unwrap();
        let mut parts = path.split('.').peekable();
        while let Some(name) = parts.next() {
            let field = def.field(name).unwrap_or_else(|| panic!("{path}"));
            if parts.peek().is_none() {
                return field.applies;
            }
            def = match &field.kind {
                Kind::Block(c) | Kind::Struct(c) => c.as_ref(),
                _ => panic!("{path}"),
            };
        }
        None
    };
    assert_eq!(at(b"nhdt", "bitmap_widgets.offset_full"), Some("frame"));
    assert_eq!(at(b"nhdt", "text_widgets.font_full"), Some("frame"));
    assert_eq!(at(b"nhdt", "bitmap_widgets.name"), None);
    assert_eq!(at(b"prt3", "color"), Some("frame"));
    assert_eq!(at(b"prt3", "postprocess.colors.color"), Some("frame"));
    assert_eq!(at(b"prt3", "death_effect"), Some("use"));
    assert_eq!(at(b"effe", "events.parts.reference"), Some("use"));
    assert_eq!(at(b"effe", "events.parts.location"), Some("use"));
    assert_eq!(at(b"effe", "events.accelerations.acceleration"), None);
    assert_eq!(at(b"ligh", "diffuse_colour_lower"), Some("frame"));
    assert_eq!(at(b"proj", "inverse_velocity_change_distance"), Some("frame"));
    assert_eq!(at(b"matg", "player_control.slowdown"), Some("frame"));
    assert_eq!(at(b"matg", "player_representations.unit"), Some("creation"));
}

/// Every `view=` in the checked-in definitions cites its source: an `E`
/// or `V` word in the meaning, or the retail reference
/// (`docs/editor/rich-editors.md`, "Colours"). The colour forms of slice
/// C2 are on their fields.
#[test]
fn builtin_view_lines_cite_their_source() {
    fn walk(
        path: &str,
        def: &StructDef,
        found: &mut Vec<(String, FieldView)>,
        bad: &mut Vec<String>,
    ) {
        for field in &def.fields {
            let here = format!("{path}.{}", field.name);
            if let Some(v) = field.view {
                found.push((here.clone(), v));
                let cited = field
                    .meaning
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|w| w == "E" || w == "V")
                    || field.meaning.contains("retail reference");
                if !cited {
                    bad.push(here.clone());
                }
            }
            if let Kind::Block(child) | Kind::Struct(child) = &field.kind {
                walk(&here, child, found, bad);
            }
        }
    }
    let defs = Defs::builtin();
    let (mut found, mut bad) = (Vec::new(), Vec::new());
    for (g, def) in &defs.groups {
        walk(&fourcc_text(*g), def, &mut found, &mut bad);
    }
    for (name, def) in &defs.structs {
        walk(name, def, &mut found, &mut bad);
    }
    assert!(bad.is_empty(), "view= without a source: {bad:?}");
    for want in [
        "deca.colour_lower",
        "deca.colour_upper",
        "lens.reflections.tint_colour",
        "fog_a.color",
        "obje.change_colors.initial_permutations.color_lower_bound",
        "obje.change_colors.initial_permutations.color_upper_bound",
        "obje.change_colors.functions.color_lower_bound",
        "obje.change_colors.functions.color_upper_bound",
    ] {
        assert!(
            found.iter().any(|(p, v)| p.ends_with(want) && *v == FieldView::Rgb),
            "{want} has view=rgb; found {found:?}"
        );
    }
}
