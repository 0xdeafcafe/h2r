//! Synthetic map-builder tests: invented groups, names and bytes only.
use crate::build::*;
use crate::*;
use std::io::Cursor;

fn cc(s: &[u8; 4]) -> FourCC {
    FourCC(*s)
}
fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

struct Tiny {
    set: TagSet,
    globals: TagId,
    scenario: TagId,
    bsp: TagId,
    lightmap: TagId,
    thing: TagId,
    raw: RawId,
}

/// globals, scenario (with one BSP descriptor), a BSP with lightmap, and a
/// "thng" tag holding a nested block, a data reference, a tag reference,
/// string IDs and raw pointers. Slot 3 is null and named.
fn tiny() -> Tiny {
    let mut set = TagSet::new(CacheType::Multiplayer, "tiny", "invented\\scenarios\\tiny");
    for g in [b"matg", b"scnr", b"sbsp", b"ltmp"] {
        set.add_group(cc(g), None, None);
    }
    set.add_group(cc(b"thng"), Some(cc(b"base")), None);
    set.add_group(cc(b"base"), None, None);
    set.strings = vec![Some(String::new()), Some("existing".into())];

    let mut globals = Block::zeroed(0x40);
    globals.put_u32(0, 0x1234_5678).string_id(4, "existing");
    let globals = set
        .add_tag(cc(b"matg"), "globals\\globals", TagPayload::Built(globals))
        .unwrap();
    // The scenario is filled in once the other tags exist.
    let scenario = set
        .add_tag(cc(b"scnr"), "invented\\scenarios\\tiny", TagPayload::Empty)
        .unwrap();
    let raw = set.add_raw(b"raw resource bytes!!".to_vec(), 0x200);

    let mut bsp = Block::zeroed(0x20);
    bsp.string_id(0, "bsp name")
        .block(8, 2, Block::new((0u8..16).collect()));
    let bsp = set
        .add_tag(cc(b"sbsp"), "invented\\bsp", TagPayload::Built(bsp))
        .unwrap();
    set.slots.push(Slot {
        name: Some("null slot keeps its name".into()),
        tag: None,
    });
    let mut light = Block::zeroed(0x10);
    light.tag_ref(0, bsp);
    let lightmap = set
        .add_tag(cc(b"ltmp"), "invented\\bsp", TagPayload::Built(light))
        .unwrap();

    let mut element = Block::zeroed(0xc);
    element.put_u32(0, 0xaaaa_aaaa).string_id(4, "new name");
    let mut elements = Block::zeroed(0x18);
    elements.put_u32(0, 7).put_u32(0xc, 9);
    elements.string_id(4, "first");
    elements.string_id(0x10, "second");
    let mut thing = Block::zeroed(0x48);
    thing
        .put_f32(0, 1.5)
        .block(4, 2, elements)
        .data(0xc, Block::new(b"data reference".to_vec()))
        .tag_ref(0x14, globals)
        .tag_ref(0x1c, TagId::NULL)
        .tag_id(0x24, bsp)
        .raw(0x28, RawRef::Local(raw))
        .raw(
            0x2c,
            RawRef::External {
                file: ResourceFile::Shared,
                offset: 0x1234,
            },
        )
        .raw(0x30, RawRef::Null)
        .block(0x34, 1, element.aligned(0x10))
        .block(0x3c, 0, Block::zeroed(0));
    let thing = set
        .add_tag(cc(b"thng"), "invented\\thing", TagPayload::Built(thing))
        .unwrap();

    let section = set.add_bsp(bsp, Some(lightmap));
    let mut descriptor = Block::zeroed(0x44);
    descriptor
        .fixup(
            0,
            FixupKind::Bsp {
                section,
                field: BspField::FileOffset,
            },
        )
        .fixup(
            4,
            FixupKind::Bsp {
                section,
                field: BspField::Size,
            },
        )
        .fixup(
            8,
            FixupKind::Bsp {
                section,
                field: BspField::Address,
            },
        )
        .tag_id(0x14, bsp)
        .tag_id(0x1c, lightmap);
    let mut scen = Block::zeroed(0x3e0);
    scen.block(0x210, 1, descriptor);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(scen);
    set.scenario = scenario;
    set.globals = globals;
    Tiny {
        set,
        globals,
        scenario,
        bsp,
        lightmap,
        thing,
        raw,
    }
}

fn read(bytes: Vec<u8>) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes)).expect("built map reads back")
}

#[test]
fn tiny_map_reads_back_through_the_reader() {
    let t = tiny();
    let (bytes, layout) = t.set.build().unwrap();
    assert_eq!(bytes.len() as u32, layout.file_size);
    let mut map = read(bytes.clone());
    map.validate_checksum().unwrap();
    let h = map.header().clone();
    assert_eq!(h.name, "tiny");
    assert_eq!(h.scenario_path, "invented\\scenarios\\tiny");
    assert_eq!(h.cache_type, CacheType::Multiplayer);
    assert_eq!(h.file_size as usize, bytes.len());
    assert_eq!(h.index_size, layout.index_size);
    assert_eq!(h.meta_size, layout.memory_size);
    // Retail layout rules: metadata ends the file, sizes are pages, index
    // at the retail base, metadata after the largest BSP.
    assert_eq!(h.index_offset + h.index_size + h.tag_data_size, h.file_size);
    assert_eq!(h.tag_data_size % PAGE, 0);
    assert_eq!(map.index_space().virtual_base, INDEX_VIRTUAL_BASE);
    assert_eq!(
        map.metadata_space().unwrap().virtual_base,
        INDEX_VIRTUAL_BASE + h.meta_size - h.tag_data_size
    );

    assert_eq!(map.index_slots(), 6);
    assert_eq!(map.scenario().unwrap().id, t.scenario);
    assert_eq!(map.globals().unwrap().id, t.globals);
    let names: Vec<_> = map.tags().iter().map(|t| t.name.clone().unwrap()).collect();
    assert_eq!(
        names,
        [
            "globals\\globals",
            "invented\\scenarios\\tiny",
            "invented\\bsp",
            "invented\\bsp",
            "invented\\thing"
        ]
    );
    assert_eq!(map.groups().len(), 6);
    let thng = map
        .groups()
        .iter()
        .find(|g| g.class == cc(b"thng"))
        .unwrap();
    assert_eq!(thng.parent, Some(cc(b"base")));
    assert_eq!(thng.grandparent, None);
    // The null slot's name survives in the table.
    let edit_names = {
        let t = h.tag_names;
        let index = map.read_file_range(t.index_offset.into(), 24).unwrap();
        word(&index, 12)
    };
    assert_ne!(edit_names, u32::MAX);

    // Existing strings keep their index; new ones follow in visit order.
    let strings: Vec<_> = map.strings().iter().map(|s| s.clone().unwrap()).collect();
    assert_eq!(
        strings,
        ["", "existing", "bsp name", "first", "second", "new name"]
    );
    assert_eq!(layout.string_count, 6);
    let fixed = map
        .read_file_range(h.fixed_strings_offset.into(), 6 * 0x80)
        .unwrap();
    assert_eq!(&fixed[0x80 * 5..0x80 * 5 + 9], b"new name\0");

    let globals = map.read_tag_bytes(t.globals).unwrap();
    assert_eq!(word(&globals, 0), 0x1234_5678);
    assert_eq!(word(&globals, 4), string_id(1, "existing").unwrap());
    assert_eq!(word(&globals, 4), 8 << 24 | 1);

    let thing_tag = map.tag(t.thing).unwrap().clone();
    let loc = thing_tag.location.unwrap();
    let thing = map.read_tag_bytes(t.thing).unwrap();
    let at = |a: u32| (a - loc.address) as usize;
    assert_eq!(f32::from_le_bytes(thing[0..4].try_into().unwrap()), 1.5);
    // Nested block: count, pointer inside this tag's payload.
    assert_eq!(word(&thing, 4), 2);
    let elements = at(word(&thing, 8));
    assert_eq!(word(&thing, elements), 7);
    assert_eq!(word(&thing, elements + 4), string_id(3, "first").unwrap());
    assert_eq!(
        word(&thing, elements + 0x10),
        string_id(4, "second").unwrap()
    );
    // Data reference.
    assert_eq!(word(&thing, 0xc), 14);
    let data = at(word(&thing, 0x10));
    assert_eq!(&thing[data..data + 14], b"data reference");
    // Tag references and a bare datum.
    assert_eq!(word(&thing, 0x14), cc(b"matg").as_u32());
    assert_eq!(TagId(word(&thing, 0x18)), t.globals);
    assert_eq!(word(&thing, 0x1c), u32::MAX);
    assert_eq!(word(&thing, 0x20), u32::MAX);
    assert_eq!(TagId(word(&thing, 0x24)), t.bsp);
    // Raw pointers: local, external selector, null.
    let local = RawPointer::decode(word(&thing, 0x28)).unwrap();
    assert_eq!(local.file, ResourceFile::Local);
    assert_eq!(local.offset, layout.raw[t.raw.0]);
    assert_eq!(local.offset % 0x200, 0);
    assert_eq!(
        map.read_file_range(local.offset.into(), 20).unwrap(),
        b"raw resource bytes!!"
    );
    let shared = RawPointer::decode(word(&thing, 0x2c)).unwrap();
    assert_eq!(shared.file, ResourceFile::Shared);
    assert_eq!(shared.offset, 0x1234);
    assert_eq!(word(&thing, 0x30), u32::MAX);
    // Aligned child and an empty block.
    let element = word(&thing, 0x38);
    assert_eq!(element % 0x10, 0);
    assert_eq!(word(&thing, at(element)), 0xaaaa_aaaa);
    assert_eq!(word(&thing, 0x34), 1);
    assert_eq!(&thing[0x3c..0x44], &[0; 8][..]);
    assert_eq!(loc.size as u64, {
        // Whole tree inside the tag's payload, padding included.
        (at(element) + 0xc) as u64
    });

    // BSP and lightmap resolve through the scenario's descriptor.
    let bsp = map.tag(t.bsp).unwrap().clone();
    let light = map.tag(t.lightmap).unwrap().clone();
    assert_eq!(bsp.index_address, 0);
    let bl = bsp.location.unwrap();
    let ll = light.location.unwrap();
    assert_eq!(bl.space.virtual_base, layout.bsp_base);
    assert_eq!(bl.address, layout.bsp_base + 16);
    assert_eq!(bl.space.file_offset, u64::from(layout.bsps[0].0));
    assert_eq!(bl.size, 0x30);
    assert_eq!(ll.address, bl.address + 0x30);
    assert_eq!(ll.size, 0x10);
    let bsp_bytes = map.read_tag_bytes(t.bsp).unwrap();
    assert_eq!(word(&bsp_bytes, 0), string_id(2, "bsp name").unwrap());
    assert_eq!(word(&bsp_bytes, 8), 2);
    let child = bl.space.translate(word(&bsp_bytes, 0xc), 16).unwrap();
    assert_eq!(
        map.read_file_range(child, 16).unwrap(),
        (0u8..16).collect::<Vec<_>>()
    );
    let light_bytes = map.read_tag_bytes(t.lightmap).unwrap();
    assert_eq!(TagId(word(&light_bytes, 4)), t.bsp);
    // BSP space sits between the index and the metadata.
    assert!(layout.bsp_base + layout.bsps[0].1 <= layout.metadata_base);
}

#[test]
fn building_is_deterministic() {
    let a = tiny().set.build().unwrap().0;
    let b = tiny().set.build().unwrap().0;
    assert_eq!(a, b);
}

#[test]
fn an_all_pinned_decomposition_rebuilds_byte_identically() {
    let first = tiny().set.build().unwrap().0;
    let mut map = read(first.clone());
    let set = TagSet::from_map(&mut map).unwrap();
    assert!(
        set.slots
            .iter()
            .flatten_tags()
            .all(|t| matches!(t.payload, TagPayload::Pinned { .. } | TagPayload::Empty))
    );
    let second = set.build().unwrap().0;
    assert_eq!(first.len(), second.len());
    assert!(first == second, "rebuild differs");
}

#[test]
fn a_pinned_tag_can_be_replaced_by_a_built_one_that_moves() {
    let t = tiny();
    let first = t.set.build().unwrap().0;
    let mut map = read(first);
    let old = map.tag(t.scenario).unwrap().location.unwrap();
    let old_bsp = map.tag(t.bsp).unwrap().location.unwrap();
    let bytes = map.read_tag_bytes(t.scenario).unwrap();
    let descriptor_at = (word(&bytes, 0x214) - old.address) as usize;
    let mut set = TagSet::from_map(&mut map).unwrap();
    // Rebuild the scenario from its known layout: root plus the BSP
    // descriptor block, whose values (pinned BSP) stay as they are.
    let mut scenario = Block::new(bytes[..0x3e0].to_vec());
    scenario.block(
        0x210,
        1,
        Block::new(bytes[descriptor_at..descriptor_at + 0x44].to_vec()),
    );
    scenario.string_id(0, "brand new");
    // Larger than before, so it cannot go back into its old range.
    scenario.data(0x30, Block::zeroed(0x100));
    let raw = set.add_raw(vec![9; 0x30], 0x1000);
    scenario.raw(4, RawRef::Local(raw));
    set.tag_mut(t.scenario).unwrap().payload = TagPayload::Built(scenario);
    let (bytes, layout) = set.build().unwrap();
    let mut map = read(bytes);
    map.validate_checksum().unwrap();
    let loc = map.tag(t.scenario).unwrap().location.unwrap();
    assert!(loc.address > old.address);
    assert_eq!(loc.size, 0x3e0 + 0x44 + 0x100);
    let b = map.read_tag_bytes(t.scenario).unwrap();
    assert_eq!(map.strings()[6].as_deref(), Some("brand new"));
    assert_eq!(word(&b, 0), string_id(6, "brand new").unwrap());
    assert_eq!(word(&b, 4), layout.raw[0]);
    assert_eq!(layout.raw[0] % 0x1000, 0);
    assert_eq!(
        map.read_file_range(layout.raw[0].into(), 0x30).unwrap(),
        vec![9; 0x30]
    );
    // The pinned BSP still resolves through the moved descriptor.
    let bsp = map.tag(t.bsp).unwrap().location.unwrap();
    assert_eq!(
        (bsp.address, bsp.size, bsp.space),
        (old_bsp.address, old_bsp.size, old_bsp.space)
    );
    assert_eq!(word(&map.read_tag_bytes(t.bsp).unwrap(), 8), 2);
    // Everything else is where it was.
    assert_eq!(
        map.tag(t.globals).unwrap().location.unwrap().address,
        layout.metadata_base
    );
    let thing = map.tag(t.thing).unwrap().location.unwrap();
    assert!(thing.address < loc.address);
}

trait FlattenTags<'a> {
    fn flatten_tags(self) -> impl Iterator<Item = &'a TagEntry>;
}
impl<'a, I: Iterator<Item = &'a Slot>> FlattenTags<'a> for I {
    fn flatten_tags(self) -> impl Iterator<Item = &'a TagEntry> {
        self.filter_map(|s| s.tag.as_ref())
    }
}

#[test]
fn invalid_sets_are_rejected() {
    let err = |set: &TagSet| set.build().unwrap_err().to_string();

    let mut t = tiny();
    t.set.groups.retain(|g| g.class != cc(b"thng"));
    assert!(err(&t.set).contains("undeclared class"));

    let mut t = tiny();
    let dangling = TagId(0xdead_0007);
    t.set
        .tag_mut(t.thing)
        .map(|e| match &mut e.payload {
            TagPayload::Built(b) => {
                b.tag_id(0x44, dangling);
            }
            _ => unreachable!(),
        })
        .unwrap();
    assert!(err(&t.set).contains("not in the set"));

    let mut t = tiny();
    if let TagPayload::Built(b) = &mut t.set.tag_mut(t.thing).unwrap().payload {
        b.string_id(0x46, "out of range");
    }
    assert!(err(&t.set).contains("outside its block"));

    let mut t = tiny();
    if let TagPayload::Built(b) = &mut t.set.tag_mut(t.thing).unwrap().payload {
        b.string_id(0x44, &"x".repeat(200));
    }
    assert!(err(&t.set).contains("fixed slot"));

    let mut t = tiny();
    let mut bad = Block::zeroed(8);
    bad.block(0, 3, Block::zeroed(8));
    t.set.tag_mut(t.thing).unwrap().payload = TagPayload::Built(bad);
    assert!(err(&t.set).contains("count does not match"));

    let mut t = tiny();
    if let TagPayload::Built(b) = &mut t.set.tag_mut(t.thing).unwrap().payload {
        b.tag_id(0x10, t.globals);
    }
    assert!(err(&t.set).contains("overlapping fixups"));

    let mut t = tiny();
    t.set.scenario = t.globals;
    assert!(err(&t.set).contains("root tag"));

    let mut t = tiny();
    t.set.name = "x".repeat(32);
    assert!(err(&t.set).contains("map name"));

    let mut t = tiny();
    if let TagPayload::Built(b) = &mut t.set.tag_mut(t.thing).unwrap().payload {
        b.raw(
            0x44,
            RawRef::External {
                file: ResourceFile::Shared,
                offset: 1 << 30,
            },
        );
    }
    assert!(err(&t.set).contains("30 bits"));

    // Slot 0 anchors the metadata; a BSP-only slot 0 cannot.
    let mut t = tiny();
    t.set.slots.swap(0, 2);
    for (i, s) in t.set.slots.iter_mut().enumerate() {
        if let Some(e) = &mut s.tag {
            e.id = TagId(e.id.0 & 0xffff_0000 | i as u32);
        }
    }
    assert!(t.set.build().is_err());

    // Too much metadata for the cache type.
    let mut t = tiny();
    t.set.cache_type = CacheType::MainMenu;
    t.set
        .add_tag(
            cc(b"thng"),
            "big",
            TagPayload::Built(Block::zeroed(0x1d3000)),
        )
        .unwrap();
    assert!(err(&t.set).contains("tag memory"));
}

#[test]
fn string_id_encoding() {
    assert_eq!(string_id(0, "").unwrap(), 0);
    assert_eq!(string_id(0x12_3456, "abc").unwrap(), 0x0312_3456);
    assert!(string_id(1 << 24, "a").is_err());
    assert!(string_id(0, &"a".repeat(256)).is_err());
    let mut set = TagSet::new(CacheType::Multiplayer, "x", "y");
    let a = set.intern("alpha").unwrap();
    assert_eq!(set.intern("alpha").unwrap(), a);
    assert_eq!(set.intern("beta").unwrap(), 4 << 24 | 1);
}

#[test]
fn new_datums_carry_slot_and_salt() {
    let mut set = TagSet::new(CacheType::Multiplayer, "x", "y");
    set.add_group(cc(b"matg"), None, None);
    let a = set.add_tag(cc(b"matg"), "a", TagPayload::Empty).unwrap();
    let b = set.add_tag(cc(b"matg"), "b", TagPayload::Empty).unwrap();
    assert_eq!(a.index(), 0);
    assert_eq!(b.index(), 1);
    assert_eq!(b.salt(), a.salt() + 1);
    assert!(set.tag(TagId(b.0 ^ 0x1_0000)).is_none());
}

#[test]
fn a_pinned_map_can_drop_its_index_tail_and_header_extras() {
    let t = tiny();
    let first = t.set.build().unwrap().0;
    // Give the source map an index tail and uninterpreted header bytes.
    let mut source = first.clone();
    let h = read(first).header().clone();
    let index_end = h.index_offset as usize + 0x20 + 6 * 12 + 6 * 16;
    source[index_end..index_end + 4].copy_from_slice(b"tail");
    source[0x144..0x148].copy_from_slice(&[1, 2, 3, 4]);
    source[0x180..0x188].copy_from_slice(&[5; 8]);
    source[0x154..0x160].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    source[0x158..0x15c].copy_from_slice(&0x800u32.to_le_bytes());
    let mut checksum = crate::checksum::XorChecksum::default();
    checksum.update(&source[0x800..]);
    let value = checksum.finish();
    source[0x2d0..0x2d4].copy_from_slice(&value.to_le_bytes());
    let mut map = read(source.clone());
    let mut set = TagSet::from_map(&mut map).unwrap();
    assert_eq!(set.build().unwrap().0, source);

    let pinned = set.pinned.as_mut().unwrap();
    assert_eq!(pinned.clear_index_tail(), 4);
    pinned.clear_header();
    let (bytes, _) = set.build().unwrap();
    let mut rebuilt = read(bytes.clone());
    rebuilt.validate_checksum().unwrap();
    assert_eq!(&bytes[index_end..index_end + 4], &[0; 4]);
    assert_eq!(&bytes[0x144..0x148], &[0; 4]);
    assert_eq!(&bytes[0x180..0x188], &[0; 8]);
    assert_eq!(rebuilt.header().low_detail_offset, 0x800);
    assert_eq!(rebuilt.header().name, "tiny");
    // Tags are untouched.
    let ids: Vec<_> = map.tags().iter().map(|t| t.id).collect();
    for id in ids {
        assert_eq!(
            map.read_tag_bytes(id).unwrap(),
            rebuilt.read_tag_bytes(id).unwrap()
        );
    }
}

#[test]
fn shared_blocks_are_placed_once() {
    let mut t = tiny();
    let child = || Block::new((0u8..32).collect()).shared(7);
    let mut a = Block::zeroed(0x10);
    a.block(0, 2, child()).block(8, 2, child());
    let mut b = Block::zeroed(0x10);
    b.block(4, 2, child());
    let a = t
        .set
        .add_tag(cc(b"thng"), "invented\\a", TagPayload::Built(a))
        .unwrap();
    let b = t
        .set
        .add_tag(cc(b"thng"), "invented\\b", TagPayload::Built(b))
        .unwrap();
    let mut map = read(t.set.build().unwrap().0);
    let ba = map.read_tag_bytes(a).unwrap();
    let bb = map.read_tag_bytes(b).unwrap();
    let target = word(&ba, 4);
    assert_eq!(word(&ba, 12), target);
    assert_eq!(word(&bb, 8), target);
    // Only the first tag holds the shared bytes.
    assert_eq!(ba.len(), 0x10 + 32);
    assert_eq!(bb.len(), 0x10);
    let loc = map.tag(a).unwrap().location.unwrap();
    assert_eq!(target, loc.address + 0x10);
    // Without keys every reference gets its own copy.
    let mut t = tiny();
    let mut c = Block::zeroed(0x10);
    c.block(0, 2, Block::new((0u8..32).collect()))
        .block(8, 2, Block::new((0u8..32).collect()));
    let c = t
        .set
        .add_tag(cc(b"thng"), "invented\\c", TagPayload::Built(c))
        .unwrap();
    let mut map = read(t.set.build().unwrap().0);
    let bc = map.read_tag_bytes(c).unwrap();
    assert_eq!(bc.len(), 0x10 + 64);
    assert_ne!(word(&bc, 4), word(&bc, 12));
}

/// Add `n` small built tags to a decomposed copy of the tiny map.
fn grown(n: usize, dirty_tail: bool, clear_tail: bool) -> (Map<Cursor<Vec<u8>>>, Vec<TagId>) {
    let mut source = read(tiny().set.build().unwrap().0);
    let mut set = TagSet::from_map(&mut source).unwrap();
    let p = set.pinned.as_mut().unwrap();
    let end = p.index_entries_end as usize;
    if dirty_tail {
        p.index[end] = 0x5a;
    }
    if clear_tail {
        p.clear_index_tail();
    }
    let ids = (0..n)
        .map(|i| {
            let mut b = Block::zeroed(12);
            b.put_u32(8, i as u32);
            set.add_tag(
                cc(b"thng"),
                &format!("invented\\grown\\{i}"),
                TagPayload::Built(b),
            )
            .unwrap()
        })
        .collect();
    (read(set.build().unwrap().0), ids)
}

fn check_grown(map: &mut Map<Cursor<Vec<u8>>>, ids: &[TagId]) -> u32 {
    let original = read(tiny().set.build().unwrap().0);
    for t in original.tags() {
        let u = map.tag(t.id).unwrap();
        assert_eq!((u.group, &u.name), (t.group, &t.name));
    }
    for (i, &id) in ids.iter().enumerate() {
        let t = map.tag(id).unwrap().clone();
        assert_eq!(
            t.name.as_deref(),
            Some(format!("invented\\grown\\{i}").as_str())
        );
        assert_eq!(word(&map.read_tag_bytes(id).unwrap(), 8), i as u32);
    }
    map.validate_checksum().unwrap();
    let h = map.header().clone();
    let index = map
        .read_file_range(h.index_offset.into(), h.index_size.into())
        .unwrap();
    word(&index, 8)
}

#[test]
fn a_pinned_index_grows_into_a_zero_tail() {
    let (mut map, ids) = grown(20, false, false);
    let t = check_grown(&mut map, &ids);
    assert!(t < INDEX_VIRTUAL_BASE + map.header().index_size);
}

#[test]
fn a_pinned_index_with_a_live_tail_moves_its_entries_to_the_metadata() {
    let (mut map, ids) = grown(20, true, false);
    let t = check_grown(&mut map, &ids);
    assert!(t >= INDEX_VIRTUAL_BASE + map.header().index_size);
    // The tail byte survives, and the map decomposes and rebuilds the same.
    let h = map.header().clone();
    let index = map
        .read_file_range(h.index_offset.into(), h.index_size.into())
        .unwrap();
    assert!(index.contains(&0x5a));
    let set = TagSet::from_map(&mut map).unwrap();
    let mut again = read(set.build().unwrap().0);
    check_grown(&mut again, &ids);
    // Clearing the tail puts the entries back in the index.
    let (mut map, ids) = grown(20, true, true);
    let t = check_grown(&mut map, &ids);
    assert!(t < INDEX_VIRTUAL_BASE + map.header().index_size);
}

#[test]
fn an_index_past_its_region_moves_its_entries_to_the_metadata() {
    let (mut map, ids) = grown(400, false, false);
    let t = check_grown(&mut map, &ids);
    assert!(t >= INDEX_VIRTUAL_BASE + map.header().index_size);
}

#[test]
fn tag_offsets_point_into_placed_tags_and_phase_holds() {
    let mut t = tiny();
    // b: 0x40 bytes, a: points at b + 0x10 and at its own + 0x20.
    let mut b = Block::new((0u8..0x40).collect());
    b.phase = 4;
    b.align = 16;
    let b = t
        .set
        .add_tag(cc(b"thng"), "invented\\b", TagPayload::Built(b))
        .unwrap();
    let a = t
        .set
        .add_tag(cc(b"thng"), "invented\\a", TagPayload::Empty)
        .unwrap();
    let mut block = Block::zeroed(0x30);
    block
        .fixup(
            0,
            FixupKind::TagOffset {
                count: 3,
                tag: b,
                offset: 0x10,
            },
        )
        .fixup(
            8,
            FixupKind::TagOffset {
                count: 1,
                tag: a,
                offset: 0x20,
            },
        );
    t.set.tag_mut(a).unwrap().payload = TagPayload::Built(block);
    let mut map = read(t.set.build().unwrap().0);
    let lb = map.tag(b).unwrap().location.unwrap();
    let la = map.tag(a).unwrap().location.unwrap();
    assert_eq!(lb.address % 16, 4);
    let ba = map.read_tag_bytes(a).unwrap();
    assert_eq!((word(&ba, 0), word(&ba, 4)), (3, lb.address + 0x10));
    assert_eq!((word(&ba, 8), word(&ba, 12)), (1, la.address + 0x20));
    // A target outside its tag is refused.
    let mut t = tiny();
    let mut bad = Block::zeroed(8);
    bad.fixup(
        0,
        FixupKind::TagOffset {
            count: 1,
            tag: t.thing,
            offset: 0x10_0000,
        },
    );
    t.set
        .add_tag(cc(b"thng"), "invented\\bad", TagPayload::Built(bad))
        .unwrap();
    assert!(t.set.build().is_err());
}

#[test]
fn an_alias_follows_its_target() {
    let mut t = tiny();
    let b = t
        .set
        .add_tag(
            cc(b"thng"),
            "invented\\target",
            TagPayload::Built(Block::zeroed(0x20)),
        )
        .unwrap();
    let a = t
        .set
        .add_tag(
            cc(b"thng"),
            "invented\\alias",
            TagPayload::Alias { tag: b, offset: 4 },
        )
        .unwrap();
    let map = read(t.set.build().unwrap().0);
    let lb = map.tag(b).unwrap().location.unwrap();
    let la = map.tag(a).unwrap();
    assert_eq!((la.index_address, la.index_size), (lb.address + 4, 0));
    let mut t = tiny();
    t.set
        .add_tag(
            cc(b"thng"),
            "invented\\alias",
            TagPayload::Alias {
                tag: t.bsp,
                offset: 0,
            },
        )
        .unwrap();
    assert!(t.set.build().is_err());
}

#[test]
fn a_header_template_and_low_detail_raw_reach_the_header() {
    let mut t = tiny();
    let mut template = vec![0u8; 0x800];
    template[0x180..0x188].copy_from_slice(&[7; 8]);
    t.set.header_template = Some(template);
    let low = t.set.add_raw(b"low detail bitmaps".to_vec(), 0x200);
    t.set.low_detail = Some(low);
    let bytes = t.set.build().unwrap().0;
    assert_eq!(bytes[0x180..0x188], [7; 8], "uninterpreted bytes kept");
    let (at, len) = (word(&bytes, 0x158) as usize, word(&bytes, 0x15c) as usize);
    assert_eq!(&bytes[at..at + len], b"low detail bitmaps");
    // The builder's own fields are written over the template.
    let map = read(bytes);
    assert_eq!(map.header().scenario_path, "invented\\scenarios\\tiny");

    t.set.header_template = Some(vec![0; 0x10]);
    assert!(t.set.build().is_err(), "a template is a whole header");
}
