//! Against synthetic guest memory: a tag index at the engine's globals,
//! a shader (`shad`), a bitmap (`bitm`) and a scenario (`scnr`), laid
//! out by the definitions' own offsets. No game data.

use super::*;
use runtime::guest::Permissions;

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const META: u32 = 0x1000_0000;

const SCNR: u32 = 0xe000_0000;
const SHAD: u32 = 0xe111_0001;
const BITM: u32 = 0xe222_0002;
const BITM2: u32 = 0xe333_0003;

const SHAD_AT: u32 = META + 0x1000;
const POST_AT: u32 = META + 0x3000;
const COLORS_AT: u32 = META + 0x4000;
const BITMAPS_AT: u32 = META + 0x5000;
const PARAMS_AT: u32 = META + 0x6000;

fn off(path: &[&str]) -> u32 {
    let mut s = Defs::builtin().group(FourCC(*b"shad")).unwrap();
    let mut at = 0;
    for (i, name) in path.iter().enumerate() {
        let f = s.field(name).unwrap_or_else(|| panic!("no {name}"));
        at = f.offset;
        if i + 1 < path.len() {
            s = match &f.kind {
                tags::defs::Kind::Block(e) | tags::defs::Kind::Struct(e) => e,
                _ => panic!("{name} has no fields"),
            };
        }
    }
    at
}

fn entry(g: &mut Guest, slot: u32, group: &[u8; 4], datum: u32, addr: u32, size: u32) {
    let e = ENTRIES + slot * 16;
    g.set_u32(e, FourCC(*group).as_u32()).unwrap();
    g.set_u32(e + 4, datum).unwrap();
    g.set_u32(e + 8, addr).unwrap();
    g.set_u32(e + 12, size).unwrap();
}

fn block(g: &mut Guest, at: u32, count: u32, addr: u32) {
    g.set_u32(at, count).unwrap();
    g.set_u32(at + 4, addr).unwrap();
}

fn level() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(META, 0x0001_0000, Permissions::RW).unwrap();
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(guest_layout::points::TAG_ENTRIES.base, ENTRIES)
        .unwrap();
    g.set_u32(HEADER + index::HEADER_SCENARIO, SCNR).unwrap();
    g.set_u32(HEADER + index::HEADER_COUNT, 5).unwrap();
    entry(&mut g, 0, b"scnr", SCNR, META, 0x100);
    entry(&mut g, 1, b"shad", SHAD, SHAD_AT, 0x54);
    entry(&mut g, 2, b"bitm", BITM, META + 0x2000, 0x4c);
    entry(&mut g, 3, b"bitm", BITM2, META + 0x2100, 0x4c);
    // An empty slot.
    g.write_bytes(ENTRIES + 4 * 16, &[0xff; 16], 1).unwrap();
    block(&mut g, SHAD_AT + off(&["postprocess"]), 1, POST_AT);
    block(&mut g, SHAD_AT + off(&["parameters"]), 1, PARAMS_AT);
    block(
        &mut g,
        POST_AT + off(&["postprocess", "colors"]),
        2,
        COLORS_AT,
    );
    block(
        &mut g,
        POST_AT + off(&["postprocess", "bitmaps"]),
        1,
        BITMAPS_AT,
    );
    g.set_u32(COLORS_AT + 4, 0xff80_4020).unwrap();
    g.set_u32(BITMAPS_AT, BITM).unwrap();
    for (i, v) in [0.25f32, 0.5, 0.75].into_iter().enumerate() {
        g.set_f32(
            PARAMS_AT + off(&["parameters", "specular_color"]) + 4 * i as u32,
            v,
        )
        .unwrap();
    }
    g.set_f32(META + 0x10, 3.0).unwrap();
    g
}

fn sel(tag: &str) -> TagSel {
    TagSel {
        tag: tag.into(),
        group: None,
    }
}

fn names() -> NameSource {
    Box::new(|_| {
        Some(Names::new(
            SCNR,
            [
                (SCNR, FourCC(*b"scnr"), "scenarios\\test\\test".to_string()),
                (SHAD, FourCC(*b"shad"), "shaders\\test\\metal".to_string()),
                (BITM, FourCC(*b"bitm"), "bitmaps\\test\\metal".to_string()),
                (BITM2, FourCC(*b"bitm"), "bitmaps\\test\\rust".to_string()),
            ],
        ))
    })
}

#[test]
fn no_map_is_unavailable() {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW)
        .unwrap();
    let e = Editor::new().tags(&g, None, None, 10).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
}

#[test]
fn lists_tags_by_group_and_name() {
    let g = level();
    let mut ed = Editor::new().with_names(names());
    let all = ed.tags(&g, None, None, 100).unwrap();
    assert_eq!(all["total"], 4, "{all}");
    assert_eq!(all["names"], true);
    let b = ed.tags(&g, Some("bitm"), None, 100).unwrap();
    assert_eq!(b["total"], 2);
    let r = ed.tags(&g, None, Some("RUST"), 100).unwrap();
    assert_eq!(r["total"], 1);
    assert_eq!(r["tags"][0]["tag"], "0xe3330003");
    assert_eq!(r["tags"][0]["defined"], true);
    let one = ed.tags(&g, None, None, 1).unwrap();
    assert_eq!(one["tags"].as_array().unwrap().len(), 1);
    assert_eq!(one["total"], 4);
}

#[test]
fn reads_a_field_and_an_element() {
    let g = level();
    let mut ed = Editor::new();
    let v = ed
        .get(&g, &sel("0xe1110001"), "postprocess[0].colors[1].color")
        .unwrap();
    assert_eq!(v["value"], 0xff80_4020u32, "{v}");
    assert_eq!(v["type"], "u32");
    assert_eq!(v["group"], "shad");
    let v = ed
        .get(&g, &sel("e1110001"), "parameters[0].specular_color")
        .unwrap();
    assert_eq!(v["value"], json!([0.25, 0.5, 0.75]));
    let v = ed
        .get(&g, &sel("0xe1110001"), "parameters[0].specular_color[2]")
        .unwrap();
    assert_eq!(v["value"], json!(0.75));
    // An element lists its fields.
    let v = ed.get(&g, &sel("0xe1110001"), "postprocess[0]").unwrap();
    let fields = v["fields"].as_array().unwrap();
    let colors = fields.iter().find(|f| f["name"] == "colors").unwrap();
    assert_eq!(colors["value"]["count"], 2, "{colors}");
    let v = ed.get(&g, &sel("0xe1110001"), "").unwrap();
    assert!(
        v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "template")
    );
}

#[test]
fn bad_paths_say_why() {
    let g = level();
    let mut ed = Editor::new();
    let e = ed
        .get(&g, &sel("0xe1110001"), "postprocess[1]")
        .unwrap_err();
    assert!(e.message.contains("has 1 elements"), "{e}");
    let e = ed
        .get(&g, &sel("0xe1110001"), "postprocess.colors")
        .unwrap_err();
    assert!(e.message.contains("index it"), "{e}");
    let e = ed.get(&g, &sel("0xe1110001"), "nope").unwrap_err();
    assert!(e.message.contains("postprocess"), "lists the fields: {e}");
    let e = ed
        .get(&g, &sel("0xe1110001"), "parameters[0].specular_color[3]")
        .unwrap_err();
    assert!(e.message.contains("3 values"), "{e}");
    let e = ed.get(&g, &sel("0xe9990009"), "").unwrap_err();
    assert_eq!(e.kind, ErrorKind::Invalid);
    // A stale salt is not the tag in that slot.
    let e = ed.get(&g, &sel("0xe9990001"), "").unwrap_err();
    assert!(e.message.contains("no tag"), "{e}");
}

#[test]
fn set_journals_and_revert_restores() {
    let mut g = level();
    let mut ed = Editor::new();
    let s = sel("0xe1110001");
    let c = ed
        .set(
            &mut g,
            &s,
            "postprocess[0].colors[1].color",
            &json!("0xff00ff00"),
        )
        .unwrap();
    assert_eq!(c["old"], 0xff80_4020u32, "{c}");
    assert_eq!(c["new"], 0xff00_ff00u32);
    assert_eq!(g.u32(COLORS_AT + 4).unwrap(), 0xff00_ff00);
    // Again: one change, the first original kept.
    ed.set(&mut g, &s, "postprocess[0].colors[1].color", &json!(1))
        .unwrap();
    ed.set(&mut g, &s, "parameters[0].specular_color[1]", &json!(2.0))
        .unwrap();
    let ch = ed.changes(&g);
    let list = ch["changes"].as_array().unwrap();
    assert_eq!(list.len(), 2, "{ch}");
    assert_eq!(list[0]["old"], 0xff80_4020u32);
    assert_eq!(list[0]["now"], 1);
    assert_eq!(list[0]["live"], true);
    assert_eq!(list[0]["stale"], false);

    // Revert one, then the rest.
    let r = ed.revert(&mut g, Some(2)).unwrap();
    assert_eq!(r["reverted"].as_array().unwrap().len(), 1);
    assert_eq!(
        g.f32(PARAMS_AT + off(&["parameters", "specular_color"]) + 4)
            .unwrap(),
        0.5
    );
    assert!(ed.revert(&mut g, Some(2)).is_err(), "gone");
    ed.revert(&mut g, None).unwrap();
    assert_eq!(g.u32(COLORS_AT + 4).unwrap(), 0xff80_4020);
    assert!(ed.changes(&g)["changes"].as_array().unwrap().is_empty());
}

#[test]
fn values_are_checked_before_anything_is_written() {
    let mut g = level();
    let mut ed = Editor::new();
    let s = sel("0xe1110001");
    assert!(
        ed.set(&mut g, &s, "parameters[0].specular_color", &json!([1, 2]))
            .is_err()
    );
    assert!(
        ed.set(&mut g, &s, "parameters[0].specular_color[0]", &json!("x"))
            .is_err()
    );
    assert!(
        ed.set(&mut g, &s, "postprocess", &json!(1)).is_err(),
        "structure"
    );
    assert!(
        ed.set(&mut g, &s, "postprocess[0]", &json!(1)).is_err(),
        "structure"
    );
    assert!(ed.changes(&g)["changes"].as_array().unwrap().is_empty());
    assert_eq!(g.u32(COLORS_AT + 4).unwrap(), 0xff80_4020);
}

#[test]
fn a_bitmap_reference_takes_only_bitmaps() {
    let mut g = level();
    let mut ed = Editor::new();
    let s = sel("0xe1110001");
    let p = "postprocess[0].bitmaps[0].bitmap";
    ed.set(&mut g, &s, p, &json!("0xe3330003")).unwrap();
    assert_eq!(g.u32(BITMAPS_AT).unwrap(), BITM2);
    let e = ed.set(&mut g, &s, p, &json!("0xe1110001")).unwrap_err();
    assert!(e.message.contains("shad"), "{e}");
    assert!(
        ed.set(&mut g, &s, p, &json!("0xe4440004")).is_err(),
        "not in the map"
    );
    ed.set(&mut g, &s, p, &Value::Null).unwrap();
    assert_eq!(g.u32(BITMAPS_AT).unwrap(), u32::MAX);
}

#[test]
fn names_select_tags() {
    let mut g = level();
    let mut ed = Editor::new().with_names(names());
    let v = ed
        .get(
            &g,
            &sel("shaders/test/METAL"),
            "postprocess[0].colors[1].color",
        )
        .unwrap();
    assert_eq!(v["name"], "shaders\\test\\metal");
    let v = ed.get(&g, &sel("bitmaps\\test\\rust.bitm"), "").unwrap();
    assert_eq!(v["tag"], "0xe3330003");
    let v = ed
        .get(
            &g,
            &TagSel {
                tag: "bitmaps\\test\\metal".into(),
                group: Some("bitm".into()),
            },
            "",
        )
        .unwrap();
    assert_eq!(v["tag"], "0xe2220002");
    assert!(ed.get(&g, &sel("nothing"), "").is_err());
    let c = ed
        .set(
            &mut g,
            &sel("shaders\\test\\metal"),
            "postprocess[0].colors[0].color",
            &json!(7),
        )
        .unwrap();
    assert_eq!(c["name"], "shaders\\test\\metal");
    // Without names, a name can't be found.
    let e = Editor::new()
        .get(&g, &sel("shaders\\test\\metal"), "")
        .unwrap_err();
    assert!(e.message.contains("by datum"), "{e}");
}

#[test]
fn a_tag_without_a_definition_takes_raw_fields() {
    let mut g = level();
    // `scnr` has a definition; an invented group at its slot has none.
    entry(&mut g, 0, b"zzzz", SCNR, META, 0x100);
    let mut ed = Editor::new();
    let s = sel("0xe0000000");
    let v = ed.get(&g, &s, "+0x10:f32").unwrap();
    assert_eq!(v["value"], json!(3.0));
    ed.set(&mut g, &s, "+0x10:f32[2]", &json!([4, 5])).unwrap();
    assert_eq!(g.f32(META + 0x14).unwrap(), 5.0);
    let e = ed.get(&g, &s, "objects").unwrap_err();
    assert!(e.message.contains("no definition"), "{e}");
}

#[test]
fn raw_blocks_reach_an_elements_fields() {
    // An invented block at scnr+0x20: 2 elements of 0x30 bytes.
    const AT: u32 = META + 0x8000;
    let mut g = level();
    block(&mut g, META + 0x20, 2, AT);
    g.set_f32(AT + 0x30 + 0x8, 7.0).unwrap();
    let mut ed = Editor::new();
    let s = sel("0xe0000000");
    let v = ed.get(&g, &s, "+0x20/0x30[1].+0x8:f32[3]").unwrap();
    assert_eq!(v["value"], json!([7.0, 0.0, 0.0]), "{v}");
    assert_eq!(v["addr"], json!(format!("{:#010x}", AT + 0x38)));
    let c = ed
        .set(&mut g, &s, "+0x20/0x30[1].+0x8:f32[3]", &json!([1, 2, 3]))
        .unwrap();
    assert_eq!(g.f32(AT + 0x30 + 0x10).unwrap(), 3.0);
    assert_eq!(c["old"], json!([7.0, 0.0, 0.0]), "{c}");
    // Exact bytes, as an editor sends a changed field.
    ed.set(&mut g, &s, "+0x20/0x30[0].+0x4:bytes[2]", &json!("beef"))
        .unwrap();
    assert_eq!(g.u16(AT + 4).unwrap(), 0xefbe);
    // The whole element, last.
    let v = ed.get(&g, &s, "+0x20/0x30[0]").unwrap();
    assert_eq!(v["type"], json!("bytes[48]"), "{v}");
    // Reverting undoes both.
    ed.revert(&mut g, None).unwrap();
    assert_eq!(g.f32(AT + 0x38).unwrap(), 7.0);
    assert_eq!(g.u16(AT + 4).unwrap(), 0);
    // The stored count bounds the index; names can't follow a raw block.
    let e = ed.get(&g, &s, "+0x20/0x30[2].+0x8:f32").unwrap_err();
    assert!(e.message.contains("has 2 elements"), "{e}");
    let e = ed.get(&g, &s, "+0x20/0x30[0].position").unwrap_err();
    assert!(e.message.contains("raw"), "{e}");
    for bad in ["+0x20/0[0].+0x8:f32", "+0x20/0x30.+0x8:f32", "+0x20"] {
        assert!(ed.get(&g, &s, bad).is_err(), "{bad}");
    }
    // In a defined group too: a raw block of the shader's root.
    block(&mut g, SHAD_AT + 0x40, 1, AT);
    let v = ed
        .get(&g, &sel("0xe1110001"), "+0x40/0x30[0].+0x38:f32")
        .unwrap();
    assert_eq!(v["value"], json!(7.0), "{v}");
}

#[test]
fn edits_from_another_map_are_never_written_back() {
    let mut g = level();
    let mut ed = Editor::new();
    ed.set(
        &mut g,
        &sel("0xe1110001"),
        "postprocess[0].colors[1].color",
        &json!(5),
    )
    .unwrap();
    // Another map: a new scenario datum (and, here, the same bytes).
    g.set_u32(HEADER + index::HEADER_SCENARIO, 0xe777_0000)
        .unwrap();
    let ch = ed.changes(&g);
    assert_eq!(ch["changes"][0]["stale"], true, "{ch}");
    let r = ed.revert(&mut g, None).unwrap();
    assert!(r["reverted"].as_array().unwrap().is_empty());
    assert_eq!(r["dropped"].as_array().unwrap().len(), 1);
    assert_eq!(g.u32(COLORS_AT + 4).unwrap(), 5, "left as it is");
}

#[test]
fn bytes_the_game_changed_since_are_left_alone() {
    let mut g = level();
    let mut ed = Editor::new();
    ed.set(
        &mut g,
        &sel("0xe1110001"),
        "postprocess[0].colors[1].color",
        &json!(5),
    )
    .unwrap();
    g.set_u32(COLORS_AT + 4, 6).unwrap();
    assert_eq!(ed.changes(&g)["changes"][0]["live"], false);
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(
        r["dropped"][0]["why"],
        "the game changed it since; left alone"
    );
    assert_eq!(g.u32(COLORS_AT + 4).unwrap(), 6);
}

#[test]
fn an_edit_passes_the_write_barrier() {
    // What the D3D device's texture and vertex caches learn of guest
    // stores from (boot's d3d write barrier drains this watch).
    let mut g = level();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, runtime::watch::CONTIGUOUS_BASE, 0x0400_0000)
        .unwrap();
    assert!(g.take_dirty(id).unwrap().is_empty());
    let mut ed = Editor::new();
    ed.set(
        &mut g,
        &sel("0xe1110001"),
        "postprocess[0].colors[1].color",
        &json!(5),
    )
    .unwrap();
    let dirty = g.take_dirty(id).unwrap();
    assert_eq!(dirty.len(), 1, "{dirty:?}");
    ed.revert(&mut g, None).unwrap();
    assert_eq!(g.take_dirty(id).unwrap().len(), 1, "a revert too");
}

#[test]
fn names_of_names_datums_in_order_and_none_without_names() {
    let g = level();
    let mut ed = Editor::new().with_names(names());
    assert_eq!(
        ed.names_of(&g, &[SHAD, 0x1234_5678, BITM2]),
        [
            Some("shaders\\test\\metal".to_owned()),
            None,
            Some("bitmaps\\test\\rust".to_owned()),
        ]
    );
    assert_eq!(Editor::new().names_of(&g, &[SHAD]), [None]);
}

/// A map where a shader and a bitmap share a name (as a weapon's `weap`,
/// `hlmt`, `mode`, `coll` and `jmad` tags share theirs).
fn shared_names() -> NameSource {
    Box::new(|_| {
        Some(Names::new(
            SCNR,
            [
                (SCNR, FourCC(*b"scnr"), "scenarios\\test\\test".to_string()),
                (SHAD, FourCC(*b"shad"), "objects\\thing".to_string()),
                (BITM, FourCC(*b"bitm"), "objects\\thing".to_string()),
                (BITM2, FourCC(*b"bitm"), "objects\\other".to_string()),
            ],
        ))
    })
}

#[test]
fn a_shared_name_resolves_among_the_wanted_groups_or_by_its_group() {
    let g = level();
    let mut ed = Editor::new().with_names(shared_names());
    let (shad, bitm) = ("shad", "bitm");
    // Any group: two tags, so the group is asked for, with the form.
    let e = ed.find_tag(&g, "objects\\thing").unwrap_err();
    assert!(
        e.message.contains("names 2 tags (shad, bitm)"),
        "{}",
        e.message
    );
    assert!(
        e.message.contains("as shad:objects\\thing"),
        "{}",
        e.message
    );
    // Among one group: the one tag of it.
    let (e, name) = ed.find_tag_in(&g, "objects\\thing", &[shad]).unwrap();
    assert_eq!((e.datum, name.as_deref()), (SHAD, Some("objects\\thing")));
    let (e, _) = ed.find_tag_in(&g, "objects/thing", &[bitm]).unwrap();
    assert_eq!(e.datum, BITM);
    // Among both: still two.
    let e = ed
        .find_tag_in(&g, "objects\\thing", &[shad, bitm])
        .unwrap_err();
    assert!(e.message.contains("give the group"), "{}", e.message);
    // None of the wanted groups: says what the name does name.
    let e = ed.find_tag_in(&g, "objects\\other", &[shad]).unwrap_err();
    assert!(
        e.message.contains("names no shad tag") && e.message.contains("bitm"),
        "{}",
        e.message
    );
    // An explicit group, as group:name or name.group, is taken as given.
    let (e, _) = ed.find_tag(&g, "bitm:objects\\thing").unwrap();
    assert_eq!(e.datum, BITM);
    let (e, _) = ed.find_tag_in(&g, "shad:objects\\thing", &[bitm]).unwrap();
    assert_eq!(e.datum, SHAD);
    let (e, _) = ed.find_tag(&g, "objects\\thing.shad").unwrap();
    assert_eq!(e.datum, SHAD);
    assert!(ed.find_tag(&g, "nope:objects\\thing").is_err());
}

#[test]
fn find_tag_resolves_a_name_or_a_datum() {
    let g = level();
    let mut ed = Editor::new().with_names(names());
    let (e, name) = ed.find_tag(&g, "Bitmaps/Test/Rust").unwrap();
    assert_eq!((e.datum, e.group), (BITM2, FourCC(*b"bitm")));
    assert_eq!(name.as_deref(), Some("bitmaps\\test\\rust"));
    let (e, name) = ed.find_tag(&g, &format!("{SHAD:#010x}")).unwrap();
    assert_eq!(
        (e.datum, name.as_deref()),
        (SHAD, Some("shaders\\test\\metal"))
    );
    assert!(ed.find_tag(&g, "nothing").is_err());
    assert!(ed.find_tag(&g, "0x12345678").is_err(), "not in the index");
}

fn byte_edit(ed: &mut Editor, g: &mut Guest, at: u32, bytes: &str) -> u64 {
    ed.batch(g, &[json!({"at": at, "value": bytes})], None)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn reverse_revert_leaves_overlapping_game_bytes_alone() {
    for separate in [false, true] {
        let mut g = level();
        let mut ed = Editor::new();
        byte_edit(&mut ed, &mut g, META, "1122");
        byte_edit(&mut ed, &mut g, META + 1, "3344");
        let last = byte_edit(&mut ed, &mut g, META + 2, "55");
        byte_edit(&mut ed, &mut g, META + 4, "66");
        // The game restores an earlier journal value. Skipping the newest
        // write also protects its wider predecessor and that one's overlap.
        g.write_bytes(META + 2, &[0x44], 1).unwrap();
        if separate {
            let r = ed.revert(&mut g, Some(last)).unwrap();
            assert_eq!(r["dropped"][0]["holds_bytes"], "44");
        }
        let r = ed.revert(&mut g, None).unwrap();
        let dropped = r["dropped"].as_array().unwrap();
        assert_eq!(dropped.len(), if separate { 2 } else { 3 });
        assert_eq!(dropped.last().unwrap()["at"], META);
        assert_eq!(dropped.last().unwrap()["holds_bytes"], "1133");
        assert_eq!(r["reverted"][0]["at"], META + 4);
        assert_eq!(r["reverted"][0]["holds_bytes"], "00");
        assert_eq!(read_bytes(&g, META, 5).unwrap(), [0x11, 0x33, 0x44, 0, 0]);
    }
}

#[test]
fn reverting_a_game_change_to_an_older_written_value_preserves_it() {
    let mut g = level();
    let mut ed = Editor::new();
    byte_edit(&mut ed, &mut g, META, "11");
    byte_edit(&mut ed, &mut g, META, "22");
    g.write_bytes(META, &[0x11], 1).unwrap();
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["dropped"].as_array().unwrap().len(), 2);
    assert_eq!(r["reverted"], json!([]));
    assert_eq!(g.u32(META).unwrap(), 0x11);
}

#[test]
fn bsp_switch_revalidates_surviving_tag_allocations_and_write_history() {
    let mut g = level();
    let mut ed = Editor::new();
    ed.at_epoch(&g, 1, 1).unwrap();
    byte_edit(&mut ed, &mut g, META, "11");
    byte_edit(&mut ed, &mut g, META, "22");
    byte_edit(&mut ed, &mut g, META + 1, "33");
    byte_edit(&mut ed, &mut g, SHAD_AT, "44");
    g.write_bytes(META + 1, &[0x55], 1).unwrap();
    // The shader datum survives, but its allocation moved.
    entry(&mut g, 1, b"shad", SHAD, META + 0x7000, 0x54);
    g.set_u32(guest_layout::points::TAG_INDEX.base, 0).unwrap();
    assert_eq!(
        ed.at_epoch(&g, 2, 1).unwrap_err().kind,
        ErrorKind::Unavailable
    );
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    ed.at_epoch(&g, 2, 1).unwrap();
    let journal = ed.changes(&g);
    let changes = journal["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[1]["fields"][0]["live"], true);
    assert_eq!(changes[2]["fields"][0]["holds"], "55");
    // Even if the game cycles back to the journal value, a change seen
    // across the switch is no longer ours to restore.
    g.write_bytes(META + 1, &[0x33], 1).unwrap();
    assert_eq!(ed.changes(&g)["changes"][2]["fields"][0]["live"], false);
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["reverted"].as_array().unwrap().len(), 2);
    assert_eq!(r["dropped"][0]["holds_bytes"], "33");
    assert_eq!(g.u32(META).unwrap(), 0x3300);
    assert_eq!(g.u32(SHAD_AT).unwrap(), 0x44);

    byte_edit(&mut ed, &mut g, META, "11");
    // A full reload of the same map can reuse every address.
    ed.at_epoch(&g, 4, 3).unwrap();
    assert_eq!(ed.changes(&g)["changes"], json!([]));
}
