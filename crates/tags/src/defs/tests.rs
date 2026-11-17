//! Definitions on invented tags, and against the checked-in schema.

use super::*;
use crate::repo::{Body, LooseTag, Node, Region, TagKey, Tree, Value};
use crate::schema::Schema;
use map::TagId;

const TEXT: &str = "\
struct base 0x8
  0x0 i16 kind default=6 -- E: test
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
";

fn defs() -> Defs {
    let mut d = Defs::default();
    d.parse_into("test", TEXT).unwrap();
    d
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
    assert_eq!(back.at("model"), r.at("model"));
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

#[test]
fn the_text_form_round_trips() {
    let d = defs();
    for (g, s) in &d.groups {
        let mut text = String::new();
        for (name, st) in &d.structs {
            text.push_str(&format!(
                "struct {name} {:#x} align {}\n",
                st.size, st.align
            ));
            st.fields_text(2, &mut text);
        }
        text.push_str(&format!(
            "group {} {:#x} align {}\n",
            fourcc_text(*g),
            s.size,
            s.align
        ));
        s.fields_text(2, &mut text);
        let mut again = Defs::default();
        again.parse_into("again", &text).unwrap();
        assert_eq!(again.group(*g), Some(s));
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
