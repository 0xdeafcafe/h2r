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
fn device_change_color_pointers_are_relocatable() {
    let schema = Schema::builtin();
    let mut inferred = schema.clone();
    for group in [FourCC(*b"ctrl"), FourCC(*b"mach")] {
        inferred.groups.get_mut(&group).unwrap().fields.remove(&0xac);

        // Reproduce the address word that fresh a30 rejected, followed
        // through four color slots and the first slot's child arrays.
        let layout = &schema.groups[&group];
        let colors: u32 = 0x80cf_e5f8;
        let address = colors - layout.size;
        let permutations = colors + 4 * 0x10;
        let functions = permutations + 0x20;
        let mut bytes = vec![0; (functions + 0x28 - address) as usize];
        for (at, count, target) in [
            (0xac, 4u32, colors),
            (layout.size, 1, permutations),
            (layout.size + 8, 1, functions),
        ] {
            let at = at as usize;
            bytes[at..at + 4].copy_from_slice(&count.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&target.to_le_bytes());
        }
        let mem = Memory::from_regions(vec![(address, bytes)], vec![]);
        let walked = walk(layout, address, &mem).unwrap();
        assert_eq!(
            walked.pointers,
            [
                (0xac, 4, colors),
                (layout.size, 1, permutations),
                (layout.size + 8, 1, functions),
            ],
            "{group}"
        );
    }
    super::infer::apply_device_change_colors(&mut inferred);
    assert_eq!(&inferred, schema);
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
fn an_emptied_field_leaves_its_array_out() {
    let (mut map, ids) = invented(3, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let mut r = Relocator::new(&schema, &mut map).unwrap();
    let id = ids[1];
    // No other tag's arrays sit inside it.
    assert!(r.pointed_into(id).is_empty());
    let full = r.to_built(id).unwrap();
    assert!(full.fixups.iter().any(|f| f.offset == 8));
    let at = map.tag(id).unwrap().location.unwrap().address;
    assert!(r.memory_mut().patch(at + 8, &[0; 8]));
    assert!(!r.memory_mut().patch(0x10, &[0; 8]));
    // The plain check refuses the tag now (its old array is uncovered);
    // the emptied build accepts it and leaves the block out.
    assert_eq!(r.check(id).unwrap_err().refusal, Refusal::Uncovered);
    let emptied = r.to_built_emptied(id).unwrap();
    // An empty block stays a zero-count field with no elements.
    assert!(emptied.fixups.iter().all(|f| f.offset != 8
        || matches!(&f.kind, map::build::FixupKind::Block { count: 0, child } if child.bytes.is_empty())));
    assert_eq!(&emptied.bytes[8..16], &[0; 8]);
    assert!(emptied.tree_len() < full.tree_len());
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

/// A pinned tag an edit grew: its 0x8 block moved to a larger array past
/// the metadata (`PinnedImage::grown`), the old array left in its bytes.
/// The plain unpin refuses it (the new array is in no tag); with the
/// grown arrays added it unpins, the new array its block, the old one
/// left out.
#[test]
fn a_grown_tag_unpins_with_its_new_array_and_without_its_old_one() {
    let (mut map, ids) = invented(1, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let id = ids[0];
    let mut set = TagSet::from_map(&mut map).unwrap();
    let TagPayload::Pinned { address, bytes } = &set.tag(id).unwrap().payload else {
        panic!("pinned");
    };
    let (at, root) = (*address, bytes.clone());
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let old = word(&root, 0xc);
    let old_elem = map
        .read_file_range(map.file_offset(old, 0x14).unwrap(), 0x14)
        .unwrap();
    // Two elements with no data, each keeping the old string ID.
    let mut grown = vec![0u8; 0x28];
    for e in 0..2 {
        grown[e * 0x14..e * 0x14 + 4].copy_from_slice(&(0x5678_0000 + e as u32).to_le_bytes());
        grown[e * 0x14 + 0x10..e * 0x14 + 0x14].copy_from_slice(&old_elem[0x10..0x14]);
    }
    let pinned = set.pinned.as_mut().unwrap();
    let new = pinned.metadata_base + pinned.metadata_size.next_multiple_of(16);
    pinned.metadata_size = new - pinned.metadata_base + grown.len() as u32;
    pinned.unowned.push((new, grown.clone()));
    pinned.grown.push((id, new));
    let TagPayload::Pinned { bytes, .. } = &mut set.tag_mut(id).unwrap().payload else {
        unreachable!()
    };
    bytes[8..0xc].copy_from_slice(&2u32.to_le_bytes());
    bytes[0xc..0x10].copy_from_slice(&new.to_le_bytes());
    assert!((at..at + root.len() as u32).contains(&old), "the old array is the tag's own");
    let mut built = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();

    let mut dst = TagSet::from_map(&mut built).unwrap();
    let mut plain = transplant::Unpinner::new(&schema, &mut built).unwrap();
    let refused = plain.unpin(&mut built, &[id], &mut dst).unwrap().unwrap_err();
    assert_eq!(refused[0].detail.refusal, Refusal::OutOfBounds);

    let mut u = transplant::Unpinner::new(&schema, &mut built).unwrap();
    u.add_grown(&mut built, [id], [(new, grown.clone())]);
    assert_eq!(u.unpin(&mut built, &[id], &mut dst).unwrap().unwrap(), 1);
    let TagPayload::Built(tree) = &dst.tag(id).unwrap().payload else {
        panic!("built");
    };
    let block = tree
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            map::build::FixupKind::Block { count, child } if f.offset == 8 => Some((*count, child)),
            _ => None,
        })
        .unwrap();
    assert_eq!(block.0, 2);
    assert_eq!(block.1.bytes.len(), grown.len());
    assert_eq!(word(&block.1.bytes, 0), 0x5678_0000);
    assert_eq!(word(&block.1.bytes, 0x14), 0x5678_0001);
}

#[test]
fn matg_locales_use_descriptor_lengths_across_a_large_raw_gap() {
    use crate::localization::{Language, Localization};
    use crate::repo::{Body, MapSource, SharedFiles};

    let matg = FourCC(*b"matg");
    let scnr = FourCC(*b"scnr");
    let mut src = TagSet::new(CacheType::Campaign, "locale", "levels\\locale");
    src.add_group(matg, None, None);
    src.add_group(scnr, None, None);
    let index = src.add_raw(vec![0, 0, 0, 1, 0, 0, 0, 0], 0x200);
    let blob = src.add_raw(b"x\0".to_vec(), 0x200);
    let mut globals = Block::zeroed(0x284);
    globals
        .put_u32(0x190, 1)
        .put_u32(0x194, 2)
        .raw(0x198, RawRef::Local(index))
        .raw(0x19c, RawRef::Local(blob));
    src.globals = src
        .add_tag(matg, "globals", TagPayload::Built(globals))
        .unwrap();
    src.scenario = src
        .add_tag(
            scnr,
            "levels\\locale",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let (_, layout) = src.build().unwrap();
    let globals_id = src.globals;
    let TagPayload::Built(globals) = &mut src.tag_mut(globals_id).unwrap().payload else {
        unreachable!();
    };
    // Retail uses a nonzero file offset even when both lengths are zero.
    // The next recognized boundary would make that a read over 64 MiB.
    globals
        .put_u32(0x278, layout.index_offset)
        .put_u32(0x27c, layout.index_offset);
    let bytes = src.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(&bytes[..])).unwrap();
    let schema = Schema::builtin();
    let mut dst = TagSet::from_map(&mut map).unwrap();
    let mut u = transplant::Unpinner::new(schema, &mut map).unwrap();
    u.add_starts([layout.index_offset + (80 << 20)]);
    assert_eq!(
        u.unpin(&mut map, &[globals_id], &mut dst).unwrap().unwrap(),
        1
    );
    assert_eq!(u.raw, (2, 10));
    assert_eq!(
        dst.raw.iter().map(|r| r.bytes.len()).collect::<Vec<_>>(),
        [8, 2, 0]
    );
    let mut rebuilt = Map::from_reader(Cursor::new(dst.build().unwrap().0)).unwrap();
    let locales = Localization::read(&mut rebuilt).unwrap();
    assert_eq!(locales.table(Language::English).bytes(0), Some(&b"x"[..]));
    assert_eq!(locales.descriptors[8].count, 0);
    assert_eq!(locales.descriptors[8].data_size, 0);

    // The dense-index build reads the same exact extents via MapSource.
    let mut shared = SharedFiles::default();
    let mut source = MapSource::new(schema, map, &mut shared).unwrap();
    let Body::Tree(globals) = source.extract(globals_id).unwrap().unwrap().body else {
        panic!("globals should extract as a tree");
    };
    assert_eq!(
        globals.raws.iter().map(|r| r.len).collect::<Vec<_>>(),
        [8, 2, 0]
    );
}

#[test]
fn a_built_copy_of_a_tag_s_root_keeps_its_blocks_once_rehomed() {
    // A conversion copies a template's root bytes and replaces some
    // fields: the rest still hold the template's addresses, which an
    // unpinned map does not keep.
    let (mut map, ids) = invented(4, false);
    let schema = Schema {
        groups: BTreeMap::from([(THNG, expected())]),
    };
    let mut set = TagSet::from_map(&mut map).unwrap();
    let mut u = transplant::Unpinner::new(&schema, &mut map).unwrap();
    u.unpin(&mut map, &ids, &mut set).unwrap().unwrap();
    let old = Memory::of(&mut map).unwrap();
    let from = map.tag(ids[1]).unwrap().location.unwrap().address;
    let root = old.slice(from, expected().size).unwrap().to_vec();
    let mut copy = map::build::Block::new(root);
    let (n, refused) = u
        .rehome(&expected(), &mut copy, 1, &mut map, &mut set)
        .unwrap();
    assert_eq!((n, refused), (1, vec![]), "the 0x8 block, once");
    // Again: nothing left to rehome, the fixup child is walked.
    assert_eq!(
        u.rehome(&expected(), &mut copy, 1, &mut map, &mut set)
            .unwrap(),
        (0, vec![])
    );
    set.tag_mut(ids[3]).unwrap().payload = TagPayload::Built(copy);
    let mut out = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    let new = Memory::of(&mut out).unwrap();
    let to = out.tag(ids[3]).unwrap().location.unwrap().address;
    let (mut x, mut y) = (
        read_tree(&expected(), from, &old).unwrap(),
        read_tree(&expected(), to, &new).unwrap(),
    );
    for tr in [&mut x, &mut y] {
        tr.bytes[0x20..0x24].fill(0);
    }
    assert_eq!(x, y, "the copy reads back as the template");
}

/// A stray pointer is the first word outside every field (any byte of a
/// field covers its word, raw fields only with `raw_ok`) whose value points
/// into the tag or a space: fields at odd offsets, overlapping, or wider
/// than a word included.
#[test]
fn stray_words_are_the_ones_no_field_touches() {
    let mem = Memory {
        spaces: vec![(0x1000, 0x2000)],
        ..Memory::default()
    };
    let own = (0x500, 0x600);
    // Every word points somewhere: the answer is the first uncovered one.
    let elem: Vec<u8> = (0..16u32)
        .flat_map(|i| (if i % 2 == 0 { 0x1000 + i } else { 0x500 + i }).to_le_bytes())
        .collect();
    let reference = |s: &Struct, raw_ok: bool| {
        let mut covered = vec![false; elem.len()];
        for (&o, f) in &s.fields {
            if raw_ok || !matches!(f, Field::Raw) {
                covered[o as usize..(o + f.width()) as usize].fill(true);
            }
        }
        (0..elem.len() - 3)
            .step_by(4)
            .find(|&k| !covered[k..k + 4].iter().any(|c| *c))
            .map(|k| (k as u32, u32_at(&elem, k)))
    };
    let tag_refs: Vec<(u32, Field)> = (0..7).map(|i| (i * 8, Field::TagRef)).collect();
    let layouts: [&[(u32, Field)]; 6] = [
        &[],
        &[(0, Field::Word)],
        &[(2, Field::Word), (8, Field::TagRef)],
        &[
            (0, Field::TagRef),
            (4, Field::Word),
            (6, Field::Raw),
            (16, Field::Word),
        ],
        &[
            (1, Field::StringId),
            (3, Field::Word),
            (20, Field::Raw),
            (24, Field::TagRef),
        ],
        &tag_refs,
    ];
    for fields in layouts {
        let s = Struct {
            size: elem.len() as u32,
            align: 4,
            fields: fields.iter().cloned().collect(),
        };
        for raw_ok in [false, true] {
            assert_eq!(
                stray_with(&s, &elem, own, &mem, raw_ok),
                reference(&s, raw_ok),
                "{fields:?} raw_ok {raw_ok}"
            );
        }
    }
}

const NHDT: FourCC = FourCC(*b"nhdt");

/// The HUD gate's threshold bytes (`nhdt` widget `+0x18`, both kinds) are
/// data, not a raw-resource pointer: the checked-in schema leaves the word
/// out, as does inference ([`super::infer::apply_engine_data`]), and no size
/// is paired with it (the text widgets' `+0x2c` is the justification).
#[test]
fn hud_thresholds_are_bytes_not_raw_pointers() {
    let widgets = |s: &Schema| -> Vec<Struct> {
        [0x8, 0x10]
            .iter()
            .map(|o| match &s.groups[&NHDT].fields[o] {
                Field::Block(c) => (**c).clone(),
                f => panic!("nhdt +{o:#x} is {f:?}"),
            })
            .collect()
    };
    for w in widgets(Schema::builtin()) {
        assert_eq!(w.fields.get(&0x18), None, "{w:?}");
        assert!(
            !w.fields
                .values()
                .any(|f| matches!(f, Field::Raw | Field::RawSize(_))),
            "{w:?}"
        );
    }
    // What inference made of them before: a raw pointer and, in the text
    // widget, its size at the justification.
    let mut inferred = Schema::builtin().clone();
    for (o, size) in [(0x8, None), (0x10, Some(0x2c))] {
        let Some(Field::Block(c)) = inferred.groups.get_mut(&NHDT).unwrap().fields.get_mut(&o)
        else {
            unreachable!()
        };
        c.fields.insert(0x18, Field::Raw);
        if let Some(at) = size {
            c.fields.insert(at, Field::RawSize(0x18));
        }
    }
    super::infer::apply_engine_data(&mut inferred);
    assert_eq!(&inferred, Schema::builtin());
}

#[test]
fn gear_curves_are_inline_floats_through_extraction_and_inference() {
    let mut inferred = Schema::builtin().clone();
    let group = FourCC(*b"vehi");
    let Some(Field::Block(gear)) = inferred.groups.get_mut(&group).unwrap().fields.get_mut(&0x268)
    else { panic!("missing gear block") };
    assert!(gear.fields.is_empty());
    for offset in [0x14, 0x1c, 0x28, 0x2c] {
        gear.fields.insert(offset, Field::Raw);
    }
    gear.fields.insert(0x10, Field::RawSize(0x14));
    gear.fields.insert(0x38, Field::Word);
    super::infer::apply_engine_data(&mut inferred);
    assert_eq!(&inferred, Schema::builtin());
}

/// The weapon slots no retail tag fills but Halo 1's weapons do (the
/// flashlight's power-on and power-off effects, the detonation effects,
/// the collision sound, the magazines' chambering effect) are tag
/// references in the checked-in schema, as inference makes them
/// ([`super::infer::apply_engine_refs`]): without, an extract read the
/// converted assault rifle's flashlight effect as bytes, with no tag.
#[test]
fn converted_slots_retail_leaves_empty_are_tag_references() {
    let schema = Schema::builtin();
    let mut inferred = schema.clone();
    for group in [b"eqip", b"garb"] {
        let header = inferred.groups.get_mut(&FourCC(*group)).unwrap();
        for at in [0x64, 0x94] {
            assert!(matches!(header.fields.remove(&at), Some(Field::Block(_))));
        }
    }
    for &(group, path, offset) in super::infer::ENGINE_REFS {
        let mut record = inferred.groups.get_mut(&group).unwrap();
        for at in path {
            let Some(Field::Block(child)) = record.fields.get_mut(at) else {
                panic!("{group} {path:x?} lacks block at {at:#x}");
            };
            record = child;
        }
        assert_eq!(record.fields.remove(&offset), Some(Field::TagRef), "{group} {path:x?} +{offset:#x}");
    }
    super::infer::apply_engine_refs(&mut inferred);
    assert_eq!(&inferred, schema);
}

/// An `nhdt` whose widgets hold thresholds at `+0x18` (Halo 1's "Reload":
/// rounds loaded below 11, `00 0b 01 00`) moves into a built map with those
/// bytes as they were, and the text widget keeps its justification. Taken
/// for a raw pointer, the word was a file offset to copy from and came out
/// as the copy's offset (`00 da 22 01` on a converted Blood Gulch).
#[test]
fn hud_thresholds_survive_an_unpinned_build() {
    let mut set = TagSet::new(CacheType::Multiplayer, "schema", "invented\\schema");
    for g in [b"matg", b"scnr", b"nhdt"] {
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
    let thresholds = [0x00, 0x0b, 0x01, 0x00];
    // Null tag references: group word zero, datum -1. The bitmap widget's
    // bitmap and shader (+0x20, +0x28); the text widget's shader (+0x20;
    // its +0x28 is a string ID).
    let mut bitmap = Block::zeroed(0x64);
    bitmap.bytes[0x18..0x1c].copy_from_slice(&thresholds);
    bitmap
        .string_id(0, "h1_warning_reload")
        .put_u32(0x24, u32::MAX)
        .put_u32(0x2c, u32::MAX);
    let mut text = Block::zeroed(0x54);
    text.bytes[0x18..0x1c].copy_from_slice(&thresholds);
    text.bytes[0x2c..0x2e].copy_from_slice(&2u16.to_le_bytes());
    text.string_id(0, "h1_weapon_number")
        .put_u32(0x24, u32::MAX);
    let mut root = Block::zeroed(0x28);
    root.bytes[4..8].fill(0xff);
    root.block(0x8, 1, bitmap).block(0x10, 1, text);
    let id = set.add_tag(NHDT, "hud", TagPayload::Built(root)).unwrap();
    let mut map = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();

    let schema = Schema {
        groups: BTreeMap::from([(NHDT, Schema::builtin().groups[&NHDT].clone())]),
    };
    let mut set = TagSet::from_map(&mut map).unwrap();
    let mut u = transplant::Unpinner::new(&schema, &mut map).unwrap();
    assert_eq!(u.unpin(&mut map, &[id], &mut set).unwrap().unwrap(), 1);
    assert_eq!(u.raw.0, 0, "no raw resource copied");
    let mut out = Map::from_reader(Cursor::new(set.build().unwrap().0)).unwrap();
    let mem = Memory::of(&mut out).unwrap();
    let a = out.tag(id).unwrap().location.unwrap().address;
    let root = mem.slice(a, 0x28).unwrap().to_vec();
    let element = |at: usize, size: u32| {
        let addr = u32::from_le_bytes(root[at + 4..at + 8].try_into().unwrap());
        mem.slice(addr, size).unwrap().to_vec()
    };
    let (bitmap, text) = (element(0x8, 0x64), element(0x10, 0x54));
    assert_eq!(bitmap[0x18..0x1c], thresholds);
    assert_eq!(text[0x18..0x1c], thresholds);
    assert_eq!(text[0x2c..0x2e], 2u16.to_le_bytes());
}

/// No retail map has an `hmt `, so its layout is the Halo 1 converter's
/// definition, whole ([`super::infer::apply_converter_groups`]): the
/// checked-in schema holds exactly that, and inference adds it back.
#[test]
fn converter_hud_messages_take_their_layout_from_the_definition() {
    let hmt = FourCC(*b"hmt ");
    let def = crate::defs::Defs::builtin().group(hmt).unwrap();
    assert_eq!(Schema::builtin().groups[&hmt], def.schema_fields().unwrap());
    let mut inferred = Schema::builtin().clone();
    inferred.groups.remove(&hmt);
    super::infer::apply_converter_groups(&mut inferred);
    assert_eq!(&inferred, Schema::builtin());
}

/// A rebuilt `hmt ` moves even when its flash colours and silhouette cache
/// key fall in the metadata address space (a10's help flash colour
/// `0x8075baff` did): those words are data, and the widget and silhouette
/// blocks past the legacy `0x94` bytes are followed.
#[test]
fn converted_hud_messages_walk_with_colours_that_look_like_addresses() {
    let hmt = FourCC(*b"hmt ");
    let address = 0x8010_0000u32;
    let (widget, silhouette) = (0xc8u32, 0xecu32);
    let mut bytes = vec![0u8; 0x104];
    let mut put = |at: u32, v: u32| bytes[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
    put(0x18, 0x4831_4857);
    put(0x20, 0x8075_baff);
    put(0x24, 0x80ff_0040);
    put(0x38, 1);
    put(0x3c, address + widget);
    put(0xb0, 1);
    put(0xb4, address + silhouette);
    put(widget + 0xc, 0x8075_baff);
    put(widget + 0x10, 0x80ff_0040);
    put(silhouette, 0x80c0_ffee);
    let mem = Memory::from_regions(vec![(address, bytes)], vec![(0x8000_0000, 0x8100_0000)]);
    let schema = Schema::builtin();
    let walked = walk(&schema.groups[&hmt], address, &mem).unwrap();
    assert_eq!(
        walked.pointers,
        [(0x38, 1, address + widget), (0xb0, 1, address + silhouette)]
    );
    assert_eq!(walked.arrays, [(0, 0xc8), (widget, 0x24), (silhouette, 0x18)]);
    // Without the help flash colour as a word, it reads as a pointer.
    let mut root = schema.groups[&hmt].clone();
    root.fields.remove(&0x20);
    let refused = walk(&root, address, &mem).unwrap_err();
    assert_eq!((refused.refusal, refused.offset, refused.value), (Refusal::UnknownPointer, 0x20, 0x8075_baff));
}
