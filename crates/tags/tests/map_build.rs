//! Retail round trips through the whole-cache builder. Opt-in: needs the
//! owner's ISO in `H2_DISC`. Nothing is written to disk; maps stay in memory.
//! `H2_BUILD_MAPS` picks maps (comma-separated stems, default
//! `lockout,01a_tutorial`).
use map::build::{TagPayload, TagSet};
use map::{FourCC, Map, RawPointer, ResourceFile, TagId};
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use tags::{DecodeMode, DecodedTag, TagData};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn open_disc_map(disc: &std::ffi::OsStr, stem: &str) -> Res<(Map<BufReader<File>>, Vec<u8>)> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc)?))?;
    let entries = iso.entries()?;
    let wanted = format!("{stem}.map");
    let entry = entries
        .iter()
        .find(|e| !e.is_dir && e.path.rsplit(['/', '\\']).next() == Some(wanted.as_str()))
        .ok_or_else(|| format!("{wanted} not on the disc"))?;
    let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
    let mut map = Map::from_region(BufReader::new(File::open(disc)?), base, entry.size.into())?;
    let mut original = Vec::new();
    map.write_to(&mut original)?;
    Ok((map, original))
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or((a.len() != b.len()).then(|| a.len().min(b.len())))
}

/// Same tags, names, groups, roots, strings and BSP placement.
fn same_index<R: Read + Seek, S: Read + Seek>(a: &Map<R>, b: &Map<S>) {
    assert_eq!(a.index_slots(), b.index_slots());
    assert_eq!(a.groups(), b.groups());
    assert_eq!(a.strings(), b.strings());
    assert_eq!(a.scenario().map(|t| t.id), b.scenario().map(|t| t.id));
    assert_eq!(a.globals().map(|t| t.id), b.globals().map(|t| t.id));
    assert_eq!(a.tags().len(), b.tags().len());
    for (x, y) in a.tags().iter().zip(b.tags()) {
        assert_eq!((x.id, x.group, &x.name), (y.id, y.group, &y.name));
        assert_eq!(x.location.is_some(), y.location.is_some(), "{}", x.id);
    }
}

fn local_bytes<R: Read + Seek>(map: &mut Map<R>, pointer: u32, size: u32) -> Option<Vec<u8>> {
    let p = RawPointer::decode(pointer)?;
    (p.file == ResourceFile::Local && size != 0)
        .then(|| map.read_file_range(p.offset.into(), size.into()).unwrap())
}

/// Records keep their source bytes, so compare what they encode to.
fn enc<T: tags::wire::BinaryRecord>(record: &T) -> Vec<u8> {
    let mut bytes = vec![0; T::SIZE];
    record.encode_into(&mut bytes).unwrap();
    bytes
}

/// Decoded bitmap equal apart from block addresses and moved LOD pointers,
/// whose pixel bytes must match instead.
fn same_bitmap<R: Read + Seek, S: Read + Seek>(
    a: &mut Map<R>,
    b: &mut Map<S>,
    x: &tags::Bitmap,
    y: &tags::Bitmap,
) -> u32 {
    let mut hx = x.header.clone();
    let mut hy = y.header.clone();
    for h in [&mut hx, &mut hy] {
        h.sequences.address = 0;
        h.images.address = 0;
    }
    assert_eq!(enc(&hx), enc(&hy), "bitmap header");
    assert_eq!(x.sequences.len(), y.sequences.len());
    for (s, t) in x.sequences.iter().zip(&y.sequences) {
        let (mut ws, mut wt) = (s.wire.clone(), t.wire.clone());
        ws.sprites.address = 0;
        wt.sprites.address = 0;
        assert_eq!(enc(&ws), enc(&wt), "bitmap sequence");
        assert_eq!(s.sprites.len(), t.sprites.len());
        for (p, q) in s.sprites.iter().zip(&t.sprites) {
            assert_eq!(enc(p), enc(q), "bitmap sprite");
        }
    }
    assert_eq!(x.images.len(), y.images.len());
    let mut compared = 0;
    for (i, j) in x.images.iter().zip(&y.images) {
        let (mut wi, mut wj) = (i.wire.clone(), j.wire.clone());
        for lod in 0..3 {
            let before = local_bytes(a, wi.lod_pointers[lod], wi.lod_sizes[lod]);
            let after = local_bytes(b, wj.lod_pointers[lod], wj.lod_sizes[lod]);
            if before.is_some() {
                assert_eq!(before, after, "LOD pixels");
                wi.lod_pointers[lod] = 0;
                wj.lod_pointers[lod] = 0;
                compared += 1;
            }
        }
        assert_eq!(enc(&wi), enc(&wj), "bitmap image");
        assert_eq!(i.low_detail, j.low_detail);
    }
    compared
}

fn round_trip(disc: &std::ffi::OsStr, stem: &str) -> Res<()> {
    let started = std::time::Instant::now();
    let (mut map, original) = open_disc_map(disc, stem)?;
    map.validate_checksum()?;
    let h = map.header();
    println!(
        "{stem}: layout: fixed strings {:#x}, string index {:#x}, string blob {:#x}+{:#x}, \
         names {:#x}+{:#x}, name index {:#x}, low detail {:#x}+{:#x}, index {:#x}+{:#x}, \
         metadata +{:#x}, file {:#x}",
        h.fixed_strings_offset,
        h.strings.index_offset,
        h.strings.data_offset,
        h.strings.data_size,
        h.tag_names.data_offset,
        h.tag_names.data_size,
        h.tag_names.index_offset,
        h.low_detail_offset,
        h.low_detail_size,
        h.index_offset,
        h.index_size,
        h.tag_data_size,
        h.file_size
    );
    {
        let h = map.header().clone();
        let index = map.read_file_range(h.index_offset.into(), h.index_size.into())?;
        let w = |at: usize| u32::from_le_bytes(index[at..at + 4].try_into().unwrap());
        let entries_end = (w(8) - 0x8006_1000) as usize + 16 * w(0x18) as usize;
        let tail = &index[entries_end..];
        let nonzero = tail.iter().filter(|&&b| b != 0).count();
        let first = tail.iter().position(|&b| b != 0);
        let last = tail.iter().rposition(|&b| b != 0);
        println!(
            "{stem}: index groups@{:#x} n={} tags@{:#x} n={} word14={:#x}; entries end {entries_end:#x}, \
             tail {} bytes, {nonzero} nonzero, first {first:x?} last {last:x?}",
            w(0),
            w(4),
            w(8),
            w(0x18),
            w(0x14),
            tail.len()
        );
        // Do metadata words point into the uninterpreted index tail?
        let space = map.metadata_space().unwrap();
        let meta = map.read_file_range(space.file_offset, space.size.into())?;
        let (lo, hi) = (0x8006_1000 + entries_end as u32, 0x8006_1000 + h.index_size);
        let mut pointing = std::collections::BTreeMap::new();
        for t in map.tags() {
            let Some(l) = t.location.filter(|l| l.space == space) else {
                continue;
            };
            let at = (l.address - space.virtual_base) as usize;
            for w in meta[at..at + l.size as usize].as_chunks::<4>().0 {
                let v = u32::from_le_bytes(*w);
                if (lo..hi).contains(&v) {
                    *pointing.entry(t.group.to_string()).or_insert(0) += 1;
                }
            }
        }
        println!(
            "{stem}: aligned metadata words inside the index tail, by tag group: {pointing:?}"
        );
        let bsps: Vec<_> = map
            .tags()
            .iter()
            .filter_map(|t| t.location)
            .filter(|l| Some(l.space) != map.metadata_space())
            .map(|l| (l.space.file_offset, l.space.size, l.space.virtual_base))
            .collect();
        println!("{stem}: BSP regions {bsps:x?}");
        let mut no_data = std::collections::BTreeMap::new();
        for t in map.tags().iter().filter(|t| t.location.is_none()) {
            *no_data.entry(t.group.to_string()).or_insert(0) += 1;
        }
        let ids: Vec<_> = map.tags().iter().take(3).map(|t| t.id).collect();
        let salts_follow_slots = map
            .tags()
            .iter()
            .find(|t| t.id.salt() != ids[0].salt().wrapping_add(t.id.index() as u16))
            .map(|t| t.id);
        println!(
            "{stem}: tags without local data by group {no_data:?}; first ids {ids:x?}; \
             first tag whose salt is not first salt + slot: {salts_follow_slots:x?}"
        );
    }
    let set = TagSet::from_map(&mut map)?;
    let pinned = set.pinned.as_ref().unwrap();
    let prefix_end = 0x800 + pinned.file.len() as u64;

    // Locale text is addressed by file offset; it must sit in the pinned prefix.
    if let Some(g) = map.globals().map(|t| t.id)
        && let TagData::Globals(globals) = DecodedTag::read(&mut map, g)?.data
    {
        for l in &globals.locales {
            if l.data_size != 0 {
                assert!(u64::from(l.data_file_offset) + u64::from(l.data_size) <= prefix_end);
            }
            if l.count != 0 {
                assert!(u64::from(l.index_file_offset) + u64::from(l.count) * 8 <= prefix_end);
            }
        }
    }

    // 1. Everything pinned: the rebuild must be the original, byte for byte.
    let (same, layout) = set.build()?;
    let diff = first_difference(&original, &same);
    println!(
        "{stem}: {} bytes, {} slots, {} strings, {} unowned metadata ranges; all-pinned rebuild {}",
        original.len(),
        set.slots.len(),
        set.strings.len(),
        pinned.unowned.len(),
        match diff {
            None => "byte-identical".to_string(),
            Some(at) => format!(
                "differs first at {at:#x} (file {:#x}, index {:#x}+{:#x}, metadata {:#x}+{:#x})",
                layout.file_size,
                layout.index_offset,
                layout.index_size,
                layout.metadata_base,
                layout.metadata_size
            ),
        }
    );
    assert_eq!(diff, None);

    // 2. Every bitmap whose payload the reviewed records cover becomes a
    //    built tree, and its local pixel data moves into new raw resources.
    //    Moving the pixels too can pass the per-type file-size limit on the
    //    biggest campaign maps; those then keep their pixels in place.
    let bitmaps: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let mut relocated = 0u32;
    for move_raw in [true, false] {
        let mut moved = TagSet::from_map(&mut map)?;
        let (mut declined, mut lods, mut raw_bytes) = (0u32, 0u32, 0u64);
        relocated = 0;
        for &id in &bitmaps {
            match tags::rebuild::relocate_bitmap(&mut moved, &mut map, id, move_raw)? {
                Some(r) => {
                    relocated += 1;
                    lods += r.moved_lods;
                    raw_bytes += r.moved_bytes;
                }
                None => declined += 1,
            }
        }
        let mut ran = 0;
        for reuse in [false, true] {
            moved.reuse_free_space = reuse;
            if let Some(bytes) = check_moved(stem, &mut map, &moved, reuse, relocated)? {
                ran += 1;
                // 3. Built trees laid back into their own space with their
                //    pixels in place reproduce the retail map exactly.
                if reuse && !move_raw {
                    assert_eq!(first_difference(&original, &bytes), None, "{stem}");
                    println!("{stem}: bitmaps as block trees, pixels in place: byte-identical");
                }
            }
        }
        println!(
            "{stem}: {} bitmaps, {relocated} rebuilt as block trees, {declined} left pinned; \
             {lods} LODs / {raw_bytes} bytes of pixels moved to new raw; {ran}/2 layouts \
             checked; {:.1?}",
            bitmaps.len(),
            started.elapsed()
        );
        assert!(
            move_raw || ran > 0,
            "{stem}: no layout fits even without moving pixels"
        );
    }
    assert!(relocated > 0);
    Ok(())
}

/// Build `moved`, read it back and compare it with `map` tag for tag.
fn check_moved<R: Read + Seek>(
    stem: &str,
    map: &mut Map<R>,
    moved: &TagSet,
    reuse: bool,
    relocated: u32,
) -> Res<Option<Vec<u8>>> {
    let (bytes, layout) = match moved.build() {
        Ok(built) => built,
        // Moving every bitmap can push a map past its type's tag-memory
        // bound (that bound is the largest retail map of the type).
        Err(map::Error::Invalid(why))
            if why.contains("tag memory") || why.contains("size limit") =>
        {
            assert!(
                !reuse || why.contains("size limit"),
                "{stem}: reusing freed space must fit"
            );
            println!(
                "{stem} [{}]: {why}; skipped",
                if reuse { "reuse" } else { "append" }
            );
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let mut rebuilt = Map::from_reader(Cursor::new(bytes.clone()))?;
    rebuilt.validate_checksum()?;
    same_index(map, &rebuilt);

    let mut moved_tags = 0;
    let mut decoded = 0;
    let mut pixel_lods = 0;
    let ids: Vec<_> = map.tags().iter().map(|t| (t.id, t.group)).collect();
    for (id, group) in ids {
        let built = matches!(
            moved.tag(id).map(|t| &t.payload),
            Some(TagPayload::Built(_))
        );
        let a = map.tag(id)?.location;
        let b = rebuilt.tag(id)?.location;
        if let (Some(a), Some(b)) = (a, b) {
            if built {
                if a.address != b.address {
                    moved_tags += 1;
                }
                assert_eq!(a.size, b.size, "{id} size");
            } else {
                assert_eq!((a.address, a.size), (b.address, b.size), "{id}");
                assert_eq!(map.read_tag_bytes(id)?, rebuilt.read_tag_bytes(id)?, "{id}");
            }
        }
        if !DecodedTag::supports(group) || a.is_none() {
            continue;
        }
        let x = DecodedTag::read_with_mode(map, id, DecodeMode::Lenient)?;
        let y = DecodedTag::read_with_mode(&mut rebuilt, id, DecodeMode::Lenient)?;
        decoded += 1;
        match (&x.data, &y.data) {
            (TagData::Bitmap(p), TagData::Bitmap(q)) if built => {
                pixel_lods += same_bitmap(map, &mut rebuilt, p, q);
            }
            _ => assert_eq!(format!("{:?}", x.data), format!("{:?}", y.data), "{id}"),
        }
    }
    let lods: u32 = moved
        .slots
        .iter()
        .filter_map(|s| match s.tag.as_ref().map(|t| &t.payload) {
            Some(TagPayload::Built(b)) => Some(b),
            _ => None,
        })
        .map(raw_fixups)
        .sum();
    assert!(pixel_lods >= lods);
    println!(
        "{stem} [{}]: {moved_tags}/{relocated} bitmaps at new addresses, {decoded} tags decoded \
         identically through crates/tags, {pixel_lods} LODs' pixels equal; rebuilt {} bytes, \
         metadata {:#x} (memory {:#x})",
        if reuse { "reuse freed space" } else { "append" },
        layout.file_size,
        layout.metadata_size,
        layout.memory_size,
    );
    if !reuse {
        assert_eq!(moved_tags, relocated);
    }
    Ok(Some(bytes))
}

fn raw_fixups(b: &map::build::Block) -> u32 {
    b.fixups
        .iter()
        .map(|f| match &f.kind {
            map::build::FixupKind::Raw(_) => 1,
            map::build::FixupKind::Block { child, .. }
            | map::build::FixupKind::Data(child)
            | map::build::FixupKind::Pointer(child) => raw_fixups(child),
            _ => 0,
        })
        .sum()
}

#[test]
#[ignore = "requires retail ISO in H2_DISC"]
fn retail_maps_rebuild_tag_for_tag() -> Res<()> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC to your retail ISO")?;
    let maps = std::env::var("H2_BUILD_MAPS").unwrap_or_else(|_| "lockout,01a_tutorial".into());
    let stems: Vec<String> = if maps == "all" {
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
        iso.entries()?
            .iter()
            .filter(|e| !e.is_dir && e.path.ends_with(".map"))
            .filter_map(|e| {
                e.path
                    .rsplit(['/', '\\'])
                    .next()?
                    .strip_suffix(".map")
                    .map(str::to_owned)
            })
            .collect()
    } else {
        maps.split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    for stem in &stems {
        round_trip(&disc, stem)?;
    }
    Ok(())
}

/// The `h2 map testbuild` variants read back through `map::Map` with the
/// same tags; "fresh" keeps every tag's bytes and drops only what it says.
#[test]
#[ignore = "requires retail ISO in H2_DISC"]
fn load_test_variants_read_back() -> Res<()> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC to your retail ISO")?;
    let stem = std::env::var("H2_BUILD_MAPS").unwrap_or_else(|_| "lockout".into());
    for stem in stem.split(',').filter(|s| !s.is_empty() && *s != "all") {
        let (mut map, original) = open_disc_map(&disc, stem)?;
        let (moved, summary) = tags::rebuild::moved_variant(&mut map)?;
        let mut built = Map::from_reader(Cursor::new(moved.build()?.0))?;
        built.validate_checksum()?;
        same_index(&map, &built);
        let mut new_addresses = 0;
        for t in map.tags().iter().filter(|t| t.group == FourCC(*b"bitm")) {
            let a = t.location.map(|l| l.address);
            if a != built.tag(t.id)?.location.map(|l| l.address) {
                new_addresses += 1;
            }
        }
        assert_eq!(new_addresses, summary.relocated);
        println!("{stem} moved: {summary:?}");

        for (keep_tail, keep_header) in [(false, false), (true, false), (false, true), (true, true)]
        {
            let (fresh, cleared) = tags::rebuild::fresh_variant(&mut map, keep_tail, keep_header)?;
            let bytes = fresh.build()?.0;
            if keep_tail && keep_header {
                assert_eq!(first_difference(&original, &bytes), None);
                continue;
            }
            let mut built = Map::from_reader(Cursor::new(bytes.clone()))?;
            built.validate_checksum()?;
            same_index(&map, &built);
            let ids: Vec<_> = map.tags().iter().map(|t| t.id).collect();
            for id in ids {
                if map.tag(id)?.location.is_some() {
                    assert_eq!(map.read_tag_bytes(id)?, built.read_tag_bytes(id)?);
                }
            }
            let h = built.header().clone();
            let index = built.read_file_range(h.index_offset.into(), h.index_size.into())?;
            let entries_end = 0x20 + 12 * map.groups().len() + 16 * map.index_slots() as usize;
            let tail_zero = index[entries_end..].iter().all(|&b| b == 0);
            assert_eq!(tail_zero, !keep_tail);
            assert_eq!(cleared > 0, !keep_tail);
            if keep_header {
                assert_eq!(bytes[0x144..0x198], original[0x144..0x198]);
            } else {
                assert!(bytes[0x174..0x198].iter().all(|&b| b == 0));
                assert!(bytes[0x144..0x148].iter().all(|&b| b == 0));
                assert!(bytes[0x20..0x120].iter().all(|&b| b == 0));
            }
            assert_eq!(h.low_detail_offset, map.header().low_detail_offset);
            println!(
                "{stem} fresh (keep tail {keep_tail}, keep header {keep_header}): reads back, \
                 {cleared} tail bytes cleared, every tag byte-identical"
            );
        }
    }
    Ok(())
}

/// A decomposed retail map gains `H2_GROW_TAGS` (default 500) new tags of a
/// new group, with the index tail kept (the entry table moves to the end of
/// the metadata) and cleared (the entries grow in place). Every old tag
/// reads back byte-identical at its address, and every new one reads back.
#[test]
#[ignore = "needs the owner's ISO"]
fn retail_maps_gain_tags() -> Res<()> {
    let Some(disc) = std::env::var_os("H2_DISC") else {
        return Ok(());
    };
    let n: usize = std::env::var("H2_GROW_TAGS").map_or(Ok(500), |v| v.parse())?;
    // Maps at the tag-memory bound (shared, mainmenu, ...) cannot grow.
    let stems = std::env::var("H2_BUILD_MAPS").unwrap_or_else(|_| "lockout,01a_tutorial".into());
    for stem in stems.split(',') {
        let (mut map, _) = open_disc_map(&disc, stem)?;
        for clear in [false, true] {
            let mut set = TagSet::from_map(&mut map)?;
            if clear {
                set.pinned.as_mut().unwrap().clear_index_tail();
            }
            let group = FourCC(*b"grow");
            set.add_group(group, None, None);
            let ids: Vec<TagId> = (0..n)
                .map(|i| {
                    let mut b = map::build::Block::zeroed(0x20);
                    b.put_u32(0, i as u32)
                        .string_id(4, &format!("grown string {i}"));
                    set.add_tag(group, &format!("grown\\tag_{i}"), TagPayload::Built(b))
                })
                .collect::<Result<_, _>>()?;
            let bytes = set.build()?.0;
            let mut grown = Map::from_reader(Cursor::new(bytes))?;
            grown.validate_checksum()?;
            let h = grown.header().clone();
            let index = grown.read_file_range(h.index_offset.into(), h.index_size.into())?;
            let t = u32::from_le_bytes(index[8..12].try_into().unwrap());
            let in_index = t < 0x8006_1000 + h.index_size;
            for old in map.tags().to_vec() {
                let new = grown.tag(old.id)?.clone();
                assert_eq!(
                    (new.group, &new.name, new.location.map(|l| l.address)),
                    (old.group, &old.name, old.location.map(|l| l.address))
                );
                if old.location.is_some() {
                    assert!(
                        map.read_tag_bytes(old.id)? == grown.read_tag_bytes(old.id)?,
                        "{}",
                        old.id
                    );
                }
            }
            for (i, id) in ids.iter().enumerate() {
                let t = grown.tag(*id)?.clone();
                assert_eq!(t.name.as_deref(), Some(format!("grown\\tag_{i}").as_str()));
                let b = grown.read_tag_bytes(*id)?;
                assert_eq!(u32::from_le_bytes(b[..4].try_into().unwrap()), i as u32);
                let sid = u32::from_le_bytes(b[4..8].try_into().unwrap());
                assert_eq!(
                    grown.strings()[(sid & 0xff_ffff) as usize].as_deref(),
                    Some(format!("grown string {i}").as_str())
                );
            }
            println!(
                "{stem}: +{n} tags ({} slots), tail {}: entries {} (table at {t:#x}), every old and new tag reads back",
                grown.index_slots(),
                if clear { "cleared" } else { "kept" },
                if in_index {
                    "in the index region"
                } else {
                    "moved to the metadata end"
                },
            );
            assert_eq!(in_index, clear);
        }
    }
    Ok(())
}
