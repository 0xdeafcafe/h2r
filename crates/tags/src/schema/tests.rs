use super::infer::{MapSample, infer};
use super::*;
use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{CacheType, FourCC, Map};
use std::io::Cursor;

const THNG: FourCC = FourCC(*b"thng");

/// An invented map: globals, scenario and `n` tags of an invented group.
/// Each `thng` root (0x30 bytes) has a block at 0x8 of `k` elements (0x14
/// bytes, a data reference at 0x4 and a string ID at 0x10), a tag reference
/// to the globals at 0x18, a raw pointer at 0x20, a bare datum (the scenario) at 0x24 and a float
/// at 0x2c. With `stray`, the last tag gets an undescribed pointer at 0x28.
fn invented(n: u32, stray: bool) -> (Map<Cursor<Vec<u8>>>, Vec<TagId>) {
    let mut set = TagSet::new(CacheType::Multiplayer, "schema", "invented\\schema");
    for g in [b"matg", b"scnr", b"thng"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals",
            TagPayload::Built(Block::zeroed(0x284)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "scenario",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let raws = [
        set.add_raw(vec![1; 0x40], 0x200),
        set.add_raw(vec![2; 0x40], 0x200),
    ];
    let mut ids = Vec::new();
    for t in 0..n {
        let k = t % 3 + 1;
        let mut elems = Block::zeroed(0x14 * k as usize);
        for e in 0..k {
            let mut data = Block::new(vec![t as u8; (e + 3) as usize]).aligned(1);
            data.bytes[0] = e as u8;
            elems
                .put_u32((e * 0x14) as usize, 0x1234_0000 + e)
                .data(e * 0x14 + 4, data)
                .string_id(e * 0x14 + 0x10, if e % 2 == 0 { "alpha" } else { "beta" });
        }
        let mut root = Block::zeroed(0x30);
        root.put_u32(0, t)
            .block(8, k, elems)
            .tag_ref(0x18, set.globals)
            .raw(0x20, RawRef::Local(raws[t as usize % 2]))
            .tag_id(0x24, set.scenario)
            .put_f32(0x2c, 1.5);
        ids.push(
            set.add_tag(THNG, &format!("thing{t}"), TagPayload::Built(root))
                .unwrap(),
        );
    }
    let bytes = set.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    if stray {
        // Point the last tag's 0x28 at its own first byte, in place.
        let last = *ids.last().unwrap();
        let loc = map.tag(last).unwrap().location.unwrap();
        let mut set = TagSet::from_map(&mut map).unwrap();
        if let TagPayload::Pinned { bytes, .. } = &mut set.tag_mut(last).unwrap().payload {
            bytes[0x28..0x2c].copy_from_slice(&loc.address.to_le_bytes());
        }
        map = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    }
    (map, ids)
}

fn expected() -> Struct {
    let data = Struct {
        size: 1,
        align: 1,
        fields: BTreeMap::new(),
    };
    let elem = Struct {
        size: 0x14,
        align: 4,
        fields: BTreeMap::from([(4, Field::Block(Box::new(data))), (0x10, Field::StringId)]),
    };
    Struct {
        size: 0x30,
        align: 4,
        fields: BTreeMap::from([
            (8, Field::Block(Box::new(elem))),
            (0x18, Field::TagRef),
            (0x20, Field::Raw),
            (0x24, Field::TagId),
        ]),
    }
}

#[test]
fn infers_blocks_data_references_tag_references_datums_and_strings() {
    let (mut map, _) = invented(6, false);
    let sample = MapSample::read("invented", &mut map).unwrap();
    let schema = infer(&mut [sample]);
    let thng = &schema.groups[&THNG];
    // Data references are 1-byte arrays; alignment is the weakest seen.
    assert_eq!(thng, &expected());
}

#[test]
fn text_form_round_trips() {
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let text = schema.to_text();
    assert!(text.starts_with(
        "group thng 0x30 align 4\n  0x8 block 0x14 align 4\n    0x4 block 0x1 align 1\n"
    ));
    assert_eq!(Schema::parse(&text).unwrap(), schema);
    assert!(Schema::parse("group thng 0x8 align 4\n  0x4 tagref\n").is_err());
    assert!(Schema::parse("  0x4 tagref\n").is_err());
    let odd = Schema {
        groups: BTreeMap::from([(
            FourCC(*b"ab c"),
            Struct {
                size: 4,
                align: 4,
                fields: BTreeMap::new(),
            },
        )]),
    };
    assert_eq!(odd.to_text(), "group 61622063 0x4 align 4\n");
    assert_eq!(Schema::parse(&odd.to_text()).unwrap(), odd);
}

#[test]
fn checked_in_schema_parses() {
    let _ = Schema::builtin();
}

#[test]
fn tags_move_and_read_back_the_same() {
    let (mut map, ids) = invented(6, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let mut set = TagSet::from_map(&mut map).unwrap();
    set.reuse_free_space = false;
    for &id in &ids {
        let block = to_built_with(&schema, &mut map, id).unwrap().unwrap();
        set.tag_mut(id).unwrap().payload = TagPayload::Built(block);
    }
    let mut moved = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    moved.validate_checksum().unwrap();
    for &id in &ids {
        let a = map.tag(id).unwrap().location.unwrap();
        let b = moved.tag(id).unwrap().location.unwrap();
        assert_ne!(a.address, b.address);
        let ta = read_tree(&expected(), a.address, &Memory::of(&mut map).unwrap()).unwrap();
        let tb = read_tree(&expected(), b.address, &Memory::of(&mut moved).unwrap()).unwrap();
        assert_eq!(ta, tb);
        assert!(!ta.children.is_empty());
    }
}

#[test]
fn an_undescribed_pointer_keeps_the_tag_pinned() {
    let (mut map, ids) = invented(4, true);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let last = *ids.last().unwrap();
    assert_eq!(
        to_built_with(&schema, &mut map, last).unwrap(),
        Err(Refusal::UnknownPointer)
    );
    assert!(to_built_with(&schema, &mut map, ids[0]).unwrap().is_ok());
    // Inference sees a pointer-like word that isn't a block (no count) and
    // claims nothing there.
    let sample = MapSample::read("invented", &mut map).unwrap();
    assert!(
        !infer(&mut [sample]).groups[&THNG]
            .fields
            .contains_key(&0x28)
    );
}

#[test]
fn a_too_small_layout_leaves_bytes_uncovered() {
    let (mut map, ids) = invented(2, false);
    let mut short = expected();
    short.fields.remove(&8);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, short)]),
    };
    // The block's count/address pair is now undescribed: an address.
    assert_eq!(
        to_built_with(&schema, &mut map, ids[0]).unwrap(),
        Err(Refusal::UnknownPointer)
    );
    assert_eq!(
        to_built_with(&Schema::default(), &mut map, ids[0]).unwrap(),
        Err(Refusal::NoSchema)
    );
}

struct Shift;
impl Remapper for Shift {
    fn datum(&mut self, d: u32) -> Option<u32> {
        // Keep even slots (shifted), drop odd ones.
        (d & 1 == 0).then_some(d + 0x10)
    }
    fn string(&mut self, v: u32) -> u32 {
        v ^ 0x0000_0100
    }
    fn raw(&mut self, v: u32, _: Option<u32>, _: &[u32]) -> RawRef {
        RawRef::External {
            file: map::ResourceFile::Shared,
            offset: v & 0xffff,
        }
    }
}

#[test]
fn remap_rewrites_ids_strings_and_raw_pointers_at_every_depth() {
    let (mut map, ids) = invented(3, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let r = Relocator::new(&schema, &mut map).unwrap();
    let tag = ids[2]; // three elements
    let before = r.to_built(tag).unwrap();
    let mut after = before.clone();
    remap(&expected(), &mut after, 1, &mut Shift);
    let w = |b: &Block, at: usize| u32::from_le_bytes(b.bytes[at..at + 4].try_into().unwrap());
    // Root: the tag reference to the globals (slot 0, even) is shifted,
    // the scenario datum (slot 1, odd) dropped, the raw pointer rewritten.
    assert_eq!(w(&after, 0x1c), w(&before, 0x1c) + 0x10);
    assert_eq!(w(&after, 0x18), w(&before, 0x18));
    assert_eq!(w(&after, 0x24), u32::MAX);
    assert_eq!(w(&after, 0x20), 0x8000_0000 | (w(&before, 0x20) & 0xffff));
    // Nested: every element's string ID.
    let child = |b: &Block| match &b.fixups.iter().find(|f| f.offset == 8).unwrap().kind {
        map::build::FixupKind::Block { child, .. } => child.clone(),
        _ => unreachable!(),
    };
    let (cb, ca) = (child(&before), child(&after));
    for e in 0..3 {
        let at = e * 0x14 + 0x10;
        assert_ne!(w(&cb, at), 0);
        assert_eq!(w(&ca, at), w(&cb, at) ^ 0x100);
    }
    // Scalars untouched.
    assert_eq!(after.bytes[..0x18], before.bytes[..0x18]);
}

#[test]
fn tags_move_in_their_source_layout() {
    let (mut map, ids) = invented(6, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let r = Relocator::new(&schema, &mut map).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    set.reuse_free_space = false;
    for &id in &ids {
        let b = r.to_built_source(id).unwrap();
        assert_eq!(
            b.bytes.len() as u32,
            map.tag(id).unwrap().location.unwrap().size
        );
        set.tag_mut(id).unwrap().payload = TagPayload::Built(b);
    }
    let mut moved = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    let (old, new) = (
        Memory::of(&mut map).unwrap(),
        Memory::of(&mut moved).unwrap(),
    );
    for &id in &ids {
        let a = map.tag(id).unwrap().location.unwrap();
        let b = moved.tag(id).unwrap().location.unwrap();
        assert_ne!(a.address, b.address);
        assert_eq!(a.size, b.size);
        assert_eq!(a.address % 16, b.address % 16);
        assert_eq!(
            read_tree(&expected(), a.address, &old).unwrap(),
            read_tree(&expected(), b.address, &new).unwrap()
        );
    }
}

#[test]
fn a_tag_tree_transplants_into_another_map() {
    let (mut src, ids) = invented(3, false);
    let (mut dst_map, _) = invented(1, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let mut dst = TagSet::from_map(&mut dst_map).unwrap();
    let before = dst.slots.len();
    // thing2 is new there; the globals and scenario it references exist
    // (same names) and are reused.
    let report = transplant::transplant(&schema, &mut src, ids[2], &mut dst)
        .unwrap()
        .unwrap();
    assert_eq!(report.added, vec![ids[2]]);
    assert_eq!(report.ids.len(), 3);
    assert_eq!(report.raw.0, 1);
    assert_eq!(dst.slots.len(), before + 1);
    let mut out = Map::from_reader(Cursor::new(dst.build().unwrap().0)).unwrap();
    let new = report.ids[&ids[2]];
    let t = out.tag(new).unwrap().clone();
    assert_eq!(t.name.as_deref(), Some("thing2"));
    let b = out.read_tag_bytes(new).unwrap();
    let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    // References point at the destination's globals and scenario.
    assert_eq!(w(0x1c), out.globals().unwrap().id.0);
    assert_eq!(w(0x24), out.scenario().unwrap().id.0);
    // The raw resource came along: thing2 used the first one (all 1s).
    let p = map::RawPointer::decode(w(0x20)).unwrap();
    assert_eq!(p.file, map::ResourceFile::Local);
    assert_eq!(
        out.read_file_range(p.offset.into(), 0x40).unwrap(),
        vec![1; 0x40]
    );
    // Same tree, address-free (string IDs here equal: both maps intern
    // "alpha" and "beta" in the same order).
    let mem = Memory::of(&mut out).unwrap();
    let old = Relocator::new(&schema, &mut src).unwrap();
    let a = read_tree(
        &expected(),
        src.tag(ids[2]).unwrap().location.unwrap().address,
        old.memory(),
    );
    let mut y = read_tree(&expected(), t.location.unwrap().address, &mem).unwrap();
    let mut x = a.unwrap();
    // The datum and raw words differ by design.
    for tr in [&mut x, &mut y] {
        tr.bytes[0x1c..0x28].fill(0);
    }
    assert_eq!(x, y);

    // A tree with a tag the schema cannot move is refused whole.
    let (mut bad, ids) = invented(3, true);
    let mut dst = TagSet::from_map(&mut dst_map).unwrap();
    let refused = transplant::transplant(&schema, &mut bad, ids[2], &mut dst)
        .unwrap()
        .unwrap_err();
    assert_eq!(refused[0].detail.refusal, Refusal::UnknownPointer);
    assert_eq!(dst.slots.len(), before);
}

#[test]
fn a_whole_map_unpins_with_its_raw_copied_and_reads_back_the_same() {
    let (mut map, ids) = invented(4, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let mut set = TagSet::from_map(&mut map).unwrap();
    let mut u = transplant::Unpinner::new(&schema, &mut map).unwrap();

    // One block field on its own: thing1's 0x8 block (two elements).
    let (count, block) = u.relocator().block_field(ids[1], 8).unwrap().unwrap();
    assert_eq!(count, 2);
    assert_eq!(block.bytes.len(), 2 * 0x14);
    assert!(
        u.relocator().block_field(ids[1], 0x18).is_err(),
        "not a block"
    );

    assert_eq!(
        u.unpin(&mut map, &ids, &mut set).unwrap().unwrap(),
        ids.len()
    );
    assert!(
        ids.iter()
            .all(|&id| matches!(set.tag(id).unwrap().payload, TagPayload::Built(_)))
    );
    // Two local resources, each copied once. Their size isn't stored, so
    // each runs to the next known start (padding included).
    assert_eq!(u.raw.0, 2);
    assert!(u.raw.1 >= 0x80);
    let mut out = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    let (old, new) = (Memory::of(&mut map).unwrap(), Memory::of(&mut out).unwrap());
    for (t, &id) in ids.iter().enumerate() {
        let a = map.tag(id).unwrap().location.unwrap().address;
        let b = out.tag(id).unwrap().location.unwrap().address;
        let (mut x, mut y) = (
            read_tree(&expected(), a, &old).unwrap(),
            read_tree(&expected(), b, &new).unwrap(),
        );
        let p =
            map::RawPointer::decode(u32::from_le_bytes(y.bytes[0x20..0x24].try_into().unwrap()))
                .unwrap();
        assert_eq!(
            out.read_file_range(p.offset.into(), 0x40).unwrap(),
            vec![1 + (t % 2) as u8; 0x40]
        );
        for tr in [&mut x, &mut y] {
            tr.bytes[0x20..0x24].fill(0);
        }
        assert_eq!(x, y, "same tree, same datums and strings");
    }
}
