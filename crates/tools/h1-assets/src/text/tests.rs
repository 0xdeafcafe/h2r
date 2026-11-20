//! Synthetic tests: invented maps and string lists, nothing from a real
//! disc. The owner's disc is read only by the `real_disc_*` tests, from
//! `$H1_ISO`.

use crate::text::convert::Locales;
use crate::text::h1::{Hmt, MessagePart, Ustr, scenario_text};
use h1map::TagId as H1TagId;
use h1map::{Group, H1Map, Tag, testing};
use map::build::{Block, TagPayload, TagSet, string_id};
use map::{CacheType, FourCC, Map, TagId};
use std::io::Cursor;

// ---------------------------------------------------------------------------
// Invented Halo 1 maps.

const SCNR_ID: u32 = testing::SCNR_ID;

/// One invented tag. `build` gets the tag's own data address and returns
/// its bytes, so address fields can be filled in consistently.
struct SynthTag {
    group: &'static [u8; 4],
    name: &'static str,
    build: Box<dyn Fn(u32) -> Vec<u8>>,
}

fn tag(
    group: &'static [u8; 4],
    name: &'static str,
    build: impl Fn(u32) -> Vec<u8> + 'static,
) -> SynthTag {
    SynthTag {
        group,
        name,
        build: Box::new(build),
    }
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A decompressed map: index header, tag array, names, then each tag's
/// data (4-aligned). The scenario is tag 0.
fn synth_map(tags: Vec<SynthTag>) -> H1Map {
    let index_len = 0x24;
    let base = testing::BASE;
    // Pass 1 measures every tag; pass 2 builds each at its real address.
    let sizes: Vec<usize> = tags.iter().map(|t| (t.build)(base).len()).collect();
    let mut meta = vec![0u8; index_len];
    let array_at = meta.len();
    meta.resize(array_at + tags.len() * h1map::TAG_ENTRY_LEN, 0);
    let addr = |off: usize| base + off as u32;
    let mut name_addrs = Vec::new();
    for t in &tags {
        name_addrs.push(addr(meta.len()));
        meta.extend(t.name.as_bytes());
        meta.push(0);
    }
    let mut data_addrs = Vec::new();
    for (i, t) in tags.iter().enumerate() {
        while !meta.len().is_multiple_of(4) {
            meta.push(0);
        }
        data_addrs.push(addr(meta.len()));
        let start = meta.len();
        meta.extend((t.build)(addr(meta.len())));
        assert_eq!(
            meta.len() - start,
            sizes[i],
            "tag size stable across passes"
        );
    }
    put(&mut meta, 0x00, addr(array_at));
    put(&mut meta, 0x04, SCNR_ID);
    put(&mut meta, 0x0c, tags.len() as u32);
    put(&mut meta, index_len - 4, h1map::TAGS);
    for (i, t) in tags.iter().enumerate() {
        let e = array_at + i * h1map::TAG_ENTRY_LEN;
        put(&mut meta, e, u32::from_be_bytes(*t.group));
        put(&mut meta, e + 4, u32::MAX);
        put(&mut meta, e + 8, u32::MAX);
        put(&mut meta, e + 0xc, SCNR_ID + i as u32);
        put(&mut meta, e + 0x10, name_addrs[i]);
        put(&mut meta, e + 0x14, data_addrs[i]);
    }
    let file_len = h1map::HEADER_LEN + meta.len();
    let mut file = vec![0u8; file_len];
    put(&mut file, 0, h1map::HEAD);
    put(&mut file, 4, h1map::VERSION);
    put(&mut file, 8, file_len as u32);
    put(&mut file, 0x10, h1map::HEADER_LEN as u32);
    put(&mut file, 0x14, meta.len() as u32);
    file[0x20..0x24].copy_from_slice(b"synt");
    put(&mut file, h1map::FOOT_OFFSET, h1map::FOOT);
    file[h1map::HEADER_LEN..].copy_from_slice(&meta);
    H1Map::from_decompressed(file).expect("invented map parses")
}

/// A scenario long enough for its text fields, every tag reference null
/// (`ffffffff`, as a null reference's ID is stored).
fn scenario() -> Vec<u8> {
    let mut s = vec![0u8; 0x5b0];
    for at in [0x574, 0x584, 0x594] {
        put(&mut s, at + 0xc, u32::MAX);
    }
    s
}

/// A `ustr` tag's bytes at address `at`: block, 20-byte elements, then the
/// strings packed, as the disc lays them out.
fn ustr_tag(strings: &[&str], at: u32) -> Vec<u8> {
    let mut elements = Vec::new();
    let mut units = Vec::new();
    let mut offsets = Vec::new();
    for s in strings {
        offsets.push(units.len() as u32);
        for u in s.encode_utf16() {
            units.extend_from_slice(&u.to_le_bytes());
        }
        units.extend_from_slice(&0u16.to_le_bytes());
        let size = 2 * (s.encode_utf16().count() + 1);
        elements.extend_from_slice(&(size as u32).to_le_bytes());
        elements.extend_from_slice(&[0; 8]); // flags, offset (below)
        elements.extend_from_slice(&[0; 8]); // address, unused
    }
    let strings_off = 12 + elements.len() as u32;
    for (i, off) in offsets.iter().enumerate() {
        let e = 12 + i * 0x14;
        let string_at = strings_off + off;
        // The offset field is tag-relative; the address is absolute.
        elements[e - 12 + 8..e - 12 + 12].copy_from_slice(&(string_at).to_le_bytes());
        elements[e - 12 + 12..e - 12 + 16].copy_from_slice(&(at + string_at).to_le_bytes());
    }
    let mut data = vec![0u8; 12];
    put(&mut data, 0, strings.len() as u32);
    put(&mut data, 4, at + 12);
    data.extend(elements);
    data.extend(units);
    data
}

/// An `hmt ` tag's bytes at address `at`, from raw parts: the blob
/// strings, the 2-byte elements (kind, value), and the messages (name,
/// element start, element count).
fn hmt_raw(
    strings: &[&str],
    elements: &[(u8, u8)],
    messages: &[(&str, u16, u32)],
    at: u32,
) -> Vec<u8> {
    let units: Vec<u16> = strings
        .iter()
        .flat_map(|s| s.encode_utf16().chain(std::iter::once(0)))
        .collect();
    let mut data = vec![0u8; 0x80];
    put(&mut data, 0, units.len() as u32 * 2);
    put(&mut data, 8, 0x80); // offset from the tag's start
    put(&mut data, 0xc, at + 0x80);
    data.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let elements_at = data.len() as u32;
    put(&mut data, 0x14, elements.len() as u32);
    put(&mut data, 0x18, at + elements_at);
    for (kind, value) in elements {
        data.push(*kind);
        data.push(*value);
    }
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let messages_at = data.len() as u32;
    put(&mut data, 0x20, messages.len() as u32);
    put(&mut data, 0x24, at + messages_at);
    for (name, start, count) in messages {
        let mut e = vec![0u8; 0x40];
        let name = name.as_bytes();
        e[..name.len()].copy_from_slice(name);
        e[0x22..0x24].copy_from_slice(&start.to_le_bytes());
        put(&mut e, 0x24, *count);
        data.extend(e);
    }
    data
}

/// One message per string, one text element per message, each message
/// named after its string.
fn hmt_simple(strings: &[&str], at: u32) -> Vec<u8> {
    let elements: Vec<(u8, u8)> = strings
        .iter()
        .map(|s| (0, (s.encode_utf16().count() + 1) as u8))
        .collect();
    let messages: Vec<(&str, u16, u32)> = strings
        .iter()
        .enumerate()
        .map(|(i, s)| (*s, i as u16, 1))
        .collect();
    hmt_raw(strings, &elements, &messages, at)
}

fn find<'a>(map: &'a H1Map, group: &[u8; 4]) -> &'a Tag {
    map.tags()
        .iter()
        .find(|t| t.group == Group::new(group))
        .expect("tag present")
}

// ---------------------------------------------------------------------------
// Reading.

#[test]
fn ustr_reads_its_strings() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"ustr", "levels\\synth\\strings", |at| {
            ustr_tag(&["Flawless Cowboy", "", "Reunion Tour"], at)
        }),
    ]);
    let ustr_tag = find(&map, b"ustr").clone();
    let ustr = Ustr::read(&map, &ustr_tag).unwrap();
    assert_eq!(ustr.strings, ["Flawless Cowboy", "", "Reunion Tour"]);
    let scenario = map.scenario().clone();
    assert!(Ustr::read(&map, &scenario).is_err());
}

#[test]
fn ustr_rejects_a_string_size_below_its_nul() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"ustr", "levels\\synth\\strings", |at| {
            let mut data = ustr_tag(&["ok", "two"], at);
            // Shrink the first string's size to its characters, no NUL.
            put(&mut data, 12, 4);
            data
        }),
    ]);
    let t = find(&map, b"ustr").clone();
    assert!(Ustr::read(&map, &t).is_err());
}

#[test]
fn hmt_reads_named_messages() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            hmt_simple(&["pickup", ">> Evade Covenant patrols", "done."], at)
        }),
    ]);
    let t = find(&map, b"hmt ").clone();
    let hmt = Hmt::read(&map, &t).unwrap();
    let got: Vec<(String, String)> = hmt
        .messages
        .iter()
        .map(|m| (m.name.clone(), m.plain_text()))
        .collect();
    assert_eq!(
        got,
        [
            ("pickup".to_string(), "pickup".to_string()),
            (
                ">> Evade Covenant patrols".to_string(),
                ">> Evade Covenant patrols".to_string()
            ),
            ("done.".to_string(), "done.".to_string()),
        ]
    );
    assert!(hmt.messages.iter().all(|m| m.parts.len() == 1));
    let scenario = map.scenario().clone();
    assert!(Hmt::read(&map, &scenario).is_err());
}

#[test]
fn hmt_reads_glyph_elements_between_text() {
    // "Click in <glyph> to zoom": two text runs and one glyph.
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            let elements = [
                (0, ("Click in ".encode_utf16().count() + 1) as u8),
                (1, 26),
                (0, (" to zoom".encode_utf16().count() + 1) as u8),
            ];
            hmt_raw(&["Click in ", " to zoom"], &elements, &[("zoom", 0, 3)], at)
        }),
    ]);
    let t = find(&map, b"hmt ").clone();
    let hmt = Hmt::read(&map, &t).unwrap();
    assert_eq!(
        hmt.messages[0].parts,
        [
            MessagePart::Text("Click in ".into()),
            MessagePart::Glyph(26),
            MessagePart::Text(" to zoom".into()),
        ]
    );
    assert_eq!(hmt.messages[0].plain_text(), "Click in  to zoom");
}

#[test]
fn hmt_rejects_elements_that_do_not_tile() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            let mut data = hmt_simple(&["one", "two"], at);
            // Point the second message at element 0 instead of 1.
            let second = data.len() - 0x40;
            data[second + 0x22..second + 0x24].copy_from_slice(&0u16.to_le_bytes());
            data
        }),
    ]);
    let t = find(&map, b"hmt ").clone();
    assert!(Hmt::read(&map, &t).is_err());
}

#[test]
fn hmt_rejects_unknown_element_types() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            hmt_raw(&["abc"], &[(2, 4)], &[("x", 0, 1)], at)
        }),
    ]);
    let t = find(&map, b"hmt ").clone();
    assert!(Hmt::read(&map, &t).is_err());
}

#[test]
fn hmt_rejects_text_that_overshoots_the_blob() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| scenario()),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            hmt_raw(&["abc"], &[(0, 5)], &[("x", 0, 1)], at)
        }),
    ]);
    let t = find(&map, b"hmt ").clone();
    assert!(Hmt::read(&map, &t).is_err());
}

/// A scenario whose text fields name the given tags (0 leaves the field
/// null).
fn scnr_with(help: u32, hud: u32) -> Vec<u8> {
    let mut s = scenario();
    if help != 0 {
        put(&mut s, 0x584, u32::from_be_bytes(*b"ustr"));
        put(&mut s, 0x584 + 0xc, help);
    }
    if hud != 0 {
        put(&mut s, 0x594, u32::from_be_bytes(*b"hmt "));
        put(&mut s, 0x594 + 0xc, hud);
    }
    s
}

#[test]
fn scenario_text_resolves_its_references() {
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| {
            scnr_with(SCNR_ID + 1, SCNR_ID + 2)
        }),
        tag(b"ustr", "levels\\synth\\help", |at| ustr_tag(&["help"], at)),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            hmt_simple(&["m"], at)
        }),
    ]);
    let text = scenario_text(&map).unwrap();
    assert_eq!(text.object_names, None);
    assert_eq!(text.help_text.expect("help ref").id, H1TagId(SCNR_ID + 1));
    assert_eq!(text.hud_messages.expect("hud ref").id, H1TagId(SCNR_ID + 2));

    // A help-text reference naming an hmt tag is refused.
    let map = synth_map(vec![
        tag(b"scnr", "levels\\synth\\synth", |_| {
            scnr_with(SCNR_ID + 1, 0)
        }),
        tag(b"hmt ", "levels\\synth\\hud messages", |at| {
            hmt_simple(&["m"], at)
        }),
    ]);
    assert!(scenario_text(&map).is_err());
}

// ---------------------------------------------------------------------------
// Converting, and reading the conversion back through Halo 2's decoders.

struct BuiltText {
    bytes: Vec<u8>,
    unic: Vec<TagId>,
}

/// Converts `lists` (tag path, strings) into a built Halo 2 map: a matg
/// with the locale index and blob as raw resources, one unic per list.
fn build_text_map(lists: &[(&str, Vec<&str>)]) -> BuiltText {
    let mut set = TagSet::new(CacheType::Campaign, "synth", "levels\\synth\\synth");
    for g in [b"matg", b"scnr", b"unic"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.strings = vec![Some(String::new())];
    let matg = set
        .add_tag(
            FourCC(*b"matg"),
            "globals\\globals",
            TagPayload::Built(Block::zeroed(0x400)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "levels\\synth\\synth",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    set.globals = matg;

    let mut locales = Locales::new();
    let mut unic = Vec::new();
    for (path, strings) in lists {
        let ustr = Ustr {
            strings: strings.iter().map(|s| s.to_string()).collect(),
        };
        let (converted, _) = locales.add_ustr(&mut set, path, &ustr).unwrap();
        unic.push(
            converted
                .install(&mut set, &format!("unic\\{path}"))
                .unwrap(),
        );
    }
    let index = set.add_raw(locales.index(), 4);
    let blob = set.add_raw(locales.blob().to_vec(), 4);

    // Pass 1 learns the raw resources' file offsets; the matg descriptor
    // holds them as plain values, so they go in and the map is built again.
    let (_, first) = set.build().unwrap();
    let mut matg_block = Block::zeroed(0x400);
    locales.patch_matg(&mut matg_block, first.raw[index.0], first.raw[blob.0]);
    set.tag_mut(matg).unwrap().payload = TagPayload::Built(matg_block);
    let (bytes, second) = set.build().unwrap();
    assert_eq!(
        first.raw, second.raw,
        "patching the matg must not move the raws"
    );
    BuiltText { bytes, unic }
}

/// The packed string ID for `name`, from the built map's name table.
fn key_of(map: &Map<Cursor<Vec<u8>>>, name: &str) -> u32 {
    let index = map
        .strings()
        .iter()
        .position(|s| s.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("string id {name:?} is in the name table"));
    string_id(index, name).unwrap()
}

#[test]
fn converted_strings_read_back_identically() {
    let lists = [
        (
            "levels\\synth\\strings",
            vec!["Flawless Cowboy", "", "» Unicode, too"],
        ),
        ("levels\\synth\\more", vec!["a", "line one\r\nline two"]),
    ];
    let built = build_text_map(&lists);
    let mut map = Map::from_reader(Cursor::new(built.bytes.clone())).unwrap();
    let localization = tags::Localization::read(&mut map).unwrap();

    let mut checked = 0;
    for (i, (path, strings)) in lists.iter().enumerate() {
        let unic = localization.unic(&mut map, built.unic[i]).unwrap();
        for (j, want) in strings.iter().enumerate() {
            let got = localization
                .text_in(
                    &unic,
                    tags::StringId(key_of(&map, &format!("{path}\\{j}"))),
                    tags::Language::English,
                )
                .unwrap()
                .unwrap_or_else(|| panic!("string {path}\\{j} resolves"));
            assert_eq!(&got, want, "string {path}\\{j}");
            checked += 1;
        }
    }
    assert_eq!(checked, 5);
}

#[test]
fn two_unic_tags_keep_their_own_rows() {
    let built = build_text_map(&[
        ("levels\\a\\strings", vec!["one", "two"]),
        ("levels\\b\\strings", vec!["three", "four"]),
    ]);
    let mut map = Map::from_reader(Cursor::new(built.bytes.clone())).unwrap();
    let localization = tags::Localization::read(&mut map).unwrap();
    let first = localization.unic(&mut map, built.unic[0]).unwrap();
    let second = localization.unic(&mut map, built.unic[1]).unwrap();
    let text = |u: &tags::Unic, path: &str, i: usize| {
        localization
            .text_in(
                u,
                tags::StringId(key_of(&map, &format!("{path}\\{i}"))),
                tags::Language::English,
            )
            .unwrap()
    };
    // Rows do not leak across ranges, in either direction.
    assert_eq!(
        text(&first, "levels\\a\\strings", 0).as_deref(),
        Some("one")
    );
    assert_eq!(text(&first, "levels\\b\\strings", 0), None);
    assert_eq!(
        text(&second, "levels\\b\\strings", 1).as_deref(),
        Some("four")
    );
    assert_eq!(text(&second, "levels\\a\\strings", 1), None);
}

// ---------------------------------------------------------------------------
// The owner's disc. Run with `cargo test -p h1text -- --ignored`.

fn real_disc() -> Option<xiso::Xiso<std::io::BufReader<std::fs::File>>> {
    // The default reaches the main checkout's disc from a worktree.
    let path = std::env::var("H1_ISO").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../../disc/halo1.iso").to_string()
    });
    let f = std::fs::File::open(&path)
        .map_err(|e| eprintln!("{path}: {e}"))
        .ok()?;
    Some(xiso::Xiso::open(std::io::BufReader::new(f)).expect("an XDVDFS image"))
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn real_disc_every_string_list_decodes() {
    let mut iso = real_disc().expect("set H1_ISO to the Halo 1 disc");
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut total_ustr = 0;
    let mut total_hmt = 0;
    let mut total_strings = 0;
    let mut total_messages = 0;
    let mut offsets_matching = 0;
    let mut offsets_off = 0;
    for entry in disc.maps.clone() {
        let name = h1map::disc::map_stem(&entry);
        let bytes = iso.read(&entry).unwrap();
        let map = H1Map::from_reader(Cursor::new(&bytes)).unwrap();
        let mut ustr_count = 0;
        let mut hmt_count = 0;
        let mut strings = 0;
        let mut messages = 0;
        let mut glyphs = 0;
        for t in map.tags() {
            match t.group {
                g if g == Group::new(b"ustr") => {
                    ustr_count += 1;
                    let ustr = Ustr::read(&map, t).expect("ustr decodes");
                    // The element offset field is the unpadded cumulative
                    // size of the earlier strings from the tag start;
                    // multiplayer and ui maps dword-pad the strings, so the
                    // address leads it there (campaign maps pad nothing, and
                    // it matches). Counted, not asserted: the reader uses the
                    // addresses.
                    let data = map.tag_data(t).unwrap();
                    let block = h1map::Block::read(data, 0).unwrap();
                    for e in map
                        .block(block, crate::text::h1::DATA_REF)
                        .unwrap()
                        .as_chunks::<{ crate::text::h1::DATA_REF }>()
                        .0
                    {
                        let address = u32::from_le_bytes(e[12..16].try_into().unwrap());
                        let off = u32::from_le_bytes(e[8..12].try_into().unwrap());
                        if address.wrapping_sub(t.data_address) == off {
                            offsets_matching += 1;
                        } else {
                            offsets_off += 1;
                        }
                    }
                    strings += ustr.strings.len();
                }
                g if g == Group::new(b"hmt ") => {
                    hmt_count += 1;
                    let hmt = Hmt::read(&map, t).expect("hmt decodes");
                    for m in &hmt.messages {
                        assert!(!m.name.is_empty(), "{name}: message name");
                        glyphs += m
                            .parts
                            .iter()
                            .filter(|p| matches!(p, MessagePart::Glyph(_)))
                            .count();
                    }
                    messages += hmt.messages.len();
                }
                _ => {}
            }
        }
        // The scenario's text fields are where every campaign map has them.
        let text = scenario_text(&map).expect("scenario text fields");
        for (field, group, r) in [
            ("object names", b"ustr", &text.object_names),
            ("help text", b"ustr", &text.help_text),
            ("hud messages", b"hmt ", &text.hud_messages),
        ] {
            if let Some(r) = r {
                let target = map.tag(r.id).unwrap();
                assert_eq!(target.group, Group::new(group), "{name}: {field}");
            }
        }
        if map.header().cache_type == h1map::CacheType::Campaign {
            assert!(text.hud_messages.is_some(), "{name}: hud messages");
            assert!(text.help_text.is_some(), "{name}: help text");
        }
        total_ustr += ustr_count;
        total_hmt += hmt_count;
        total_strings += strings;
        total_messages += messages;
        eprintln!(
            "{name:<4} ustr {ustr_count:>3} ({strings:>4} strings)  hmt {hmt_count} ({messages:>3} messages, {glyphs} glyphs)"
        );
    }
    eprintln!(
        "total: {total_ustr} ustr ({total_strings} strings), {total_hmt} hmt ({total_messages} messages); element offsets: {offsets_matching} tag-relative, {offsets_off} off"
    );
    assert!(total_ustr > 0 && total_hmt > 0);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn real_disc_a30_converts_and_reads_back_through_halo_2() {
    let mut iso = real_disc().expect("set H1_ISO to the Halo 1 disc");
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let entry = disc.map("a30").expect("a30 on the disc");
    let bytes = iso.read(entry).unwrap();
    let map = H1Map::from_reader(Cursor::new(&bytes)).unwrap();

    let mut set = TagSet::new(CacheType::Campaign, "a30", "levels\\a30\\a30");
    for g in [b"matg", b"scnr", b"unic"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.strings = vec![Some(String::new())];
    let matg = set
        .add_tag(
            FourCC(*b"matg"),
            "globals\\globals",
            TagPayload::Built(Block::zeroed(0x400)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "levels\\a30\\a30",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    set.globals = matg;

    let mut locales = Locales::new();
    let mut rows: Vec<(TagId, Vec<(u32, String)>)> = Vec::new();
    for t in map.tags() {
        let (texts, ids) = match t.group {
            g if g == Group::new(b"ustr") => {
                let ustr = Ustr::read(&map, t).unwrap();
                let ids = {
                    let (converted, ids) = locales.add_ustr(&mut set, &t.name, &ustr).unwrap();
                    let id = converted
                        .install(&mut set, &format!("unic\\{}", t.name))
                        .unwrap();
                    rows.push((id, Vec::new()));
                    ids
                };
                (ustr.strings, ids)
            }
            g if g == Group::new(b"hmt ") => {
                let hmt = Hmt::read(&map, t).unwrap();
                let ids = {
                    let (converted, ids) = locales.add_hmt(&mut set, &t.name, &hmt).unwrap();
                    let id = converted
                        .install(&mut set, &format!("unic\\{}", t.name))
                        .unwrap();
                    rows.push((id, Vec::new()));
                    ids
                };
                (hmt.messages.iter().map(|m| m.plain_text()).collect(), ids)
            }
            _ => continue,
        };
        let slot = rows.last_mut().expect("just pushed");
        slot.1 = ids.iter().map(|k| k.0).zip(texts).collect();
    }
    assert!(rows.len() > 1, "a30 has string lists");
    eprintln!("a30: {} unic tags, {} rows", rows.len(), locales.len());

    let index = set.add_raw(locales.index(), 4);
    let blob = set.add_raw(locales.blob().to_vec(), 4);
    let (_, first) = set.build().unwrap();
    let mut matg_block = Block::zeroed(0x400);
    locales.patch_matg(&mut matg_block, first.raw[index.0], first.raw[blob.0]);
    set.tag_mut(matg).unwrap().payload = TagPayload::Built(matg_block);
    let (built, second) = set.build().unwrap();
    assert_eq!(first.raw, second.raw);

    let mut h2 = Map::from_reader(Cursor::new(built)).unwrap();
    let localization = tags::Localization::read(&mut h2).unwrap();
    let mut checked = 0;
    for (id, list) in &rows {
        let unic = localization.unic(&mut h2, *id).unwrap();
        for (key, text) in list {
            let got = localization
                .text_in(&unic, tags::StringId(*key), tags::Language::English)
                .unwrap()
                .unwrap_or_else(|| panic!("row {key:#x} resolves"));
            assert_eq!(&got, text, "row {key:#x}");
            checked += 1;
        }
    }
    eprintln!("a30: {checked} strings read back identical");
    assert!(checked > 100);
}

#[test]
fn an_extended_table_keeps_the_base_rows_first() {
    let mut set = TagSet::new(CacheType::Campaign, "synth", "levels\\synth\\synth");
    set.strings = vec![Some(String::new())];
    // A base table of two rows: "one", "two".
    let mut index = Vec::new();
    for (key, at) in [(0x0100_0001u32, 0u32), (0x0100_0002, 4)] {
        index.extend_from_slice(&key.to_le_bytes());
        index.extend_from_slice(&at.to_le_bytes());
    }
    let blob = b"one\0two\0".to_vec();
    let mut locales = Locales::extending(&index, blob.clone()).unwrap();
    assert_eq!(locales.len(), 2);
    let ustr = Ustr {
        strings: vec!["three".into()],
    };
    let (unic, ids) = locales.add_ustr(&mut set, "h1\\list", &ustr).unwrap();
    // The base rows and blob are untouched; the new row follows them, and
    // the unic's English range starts after the base rows.
    assert_eq!(&locales.index()[..16], &index[..]);
    assert_eq!(&locales.index()[16..20], &ids[0].0.to_le_bytes());
    assert_eq!(&locales.index()[20..24], &8u32.to_le_bytes());
    assert_eq!(locales.blob(), b"one\0two\0three\0");
    assert_eq!(&unic.root.bytes[0x10..0x14], &[2, 0, 1, 0]);
    // Malformed tables are refused.
    assert!(Locales::extending(&index[..12], blob.clone()).is_err());
    assert!(Locales::extending(&index, b"x\0".to_vec()).is_err());
}

#[test]
fn mission_hud_rebuild_links_text_and_extends_pinned_locales() {
    use crate::text::hud::{GROUP, MARKER, MARKER_AT, SCENARIO_HMT};
    use crate::text::scenario::{LevelText, unicode_path};

    let h1 = synth_map(vec![
        tag(b"scnr", "levels\\synthetic", |_| {
            let mut bytes = scnr_with(SCNR_ID + 1, SCNR_ID + 2);
            put(&mut bytes, 0x574, u32::from_be_bytes(*b"ustr"));
            put(&mut bytes, 0x580, SCNR_ID + 1);
            bytes
        }),
        tag(b"ustr", "help", |at| ustr_tag(&["Walk slowly with %move"], at)),
        tag(b"hmt ", "mission", |at| {
            hmt_raw(
                &["Try ", " next", "Reach the door"],
                &[(0, 5), (1, 26), (0, 6), (0, 15)],
                &[("hint", 0, 3), ("objective", 3, 1)],
                at,
            )
        }),
        tag(b"hudg", "globals", |_| vec![0; 0xe8]),
    ]);
    let base = build_text_map(&[("base", vec!["Already here"])]);
    let mut map = Map::from_reader(Cursor::new(base.bytes)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let mut locales = Locales::from_map(&mut map).unwrap();
    assert_eq!(locales.len(), 1);

    let level = LevelText::install(&h1, &mut set, &mut locales).unwrap();
    assert_eq!(level.message_index("OBJECTIVE"), Some(1));
    assert_eq!(level.message_index("missing"), None);
    let mut scenario = Block::zeroed(0x3e0);
    level.write(&mut scenario);
    set.tag_mut(set.scenario).unwrap().payload = TagPayload::Built(scenario);
    locales.install_pinned(&mut set).unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();

    let root = map.read_tag_bytes(set.scenario).unwrap();
    let word = |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(word(&root, SCENARIO_HMT as usize), GROUP.as_u32());
    let hud = TagId(word(&root, SCENARIO_HMT as usize + 4));
    let bytes = map.read_tag_bytes(hud).unwrap();
    assert_eq!(word(&bytes, MARKER_AT as usize), MARKER);
    assert_eq!(word(&bytes, 8), 6);
    assert_eq!(word(&bytes, 0x10), 3);
    assert_eq!(word(&bytes, crate::text::hud::SCENARIO_HELP as usize), 0x0001_0002);
    let messages = map.file_offset(word(&bytes, 0x14), 0x80).unwrap();
    let entries = map.read_file_range(messages, 0x80).unwrap();
    assert_eq!(&entries[0x60..0x65], &[11, 0, 3, 0, 1]);

    let localization = tags::Localization::read(&mut map).unwrap();
    let english = localization.table(tags::Language::English);
    assert_eq!(english.bytes(0), Some("Already here".as_bytes()));
    let name = unicode_path("mission");
    let tag = map
        .tag(map::TagQuery::NameInGroup(&name, FourCC(*b"unic")))
        .unwrap()
        .id;
    let unic = localization.unic(&mut map, tag).unwrap();
    let key = tags::StringId(key_of(&map, "mission\\hint"));
    assert_eq!(
        localization
            .text_in(&unic, key, tags::Language::English)
            .unwrap()
            .as_deref(),
        Some("Try \u{e451} next"),
    );
}
