//! Tag reads on the host (`super::super::world::map_tags`): the start gate
//! blocks a script whose tag or field the map lacks, a script that passes
//! reads the field from the tag's data in invented guest memory, and a
//! swapped-in script is held to the same check.

use ::rules::ty::fixed;
use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, index_layout};
use map::FourCC;

use super::*;
use crate::rules::world::map_tags::TagIndex;

const INDEX: u32 = 0x1010_0000;
const ENTRIES: u32 = 0x1010_1000;
const DATA: u32 = 0x1010_2000;
const ROCK: u32 = 0xe001_0002;
const PATH: &str = "objects/props/rock";

const READS: &str = r#"
rules "Tag Reads" 1
type slayer
global var radius: fixed = 0.0
global var message: int = 0
on tick {
    radius = tag_fixed("objects/props/rock", "object.bounding_radius")
    message = tag_int("objects/props/rock", "object.hud_message_index")
}
"#;

fn compile(src: &str) -> Ir {
    ::rules::compile(src).unwrap()
}

/// The map's index with the rock, a scenery tag, under a backslash path as
/// the map file spells it.
fn rock() -> TagIndex {
    TagIndex::new([("objects\\props\\rock".to_string(), FourCC(*b"scen"), ROCK)])
}

/// A started game whose map has `index`, with the rock loaded in guest
/// memory: a 1.5 bounding radius and a message index of -3.
fn start(src: &str, index: TagIndex) -> (FakeGuest, Arc<Shared>, String) {
    let ir = compile(src);
    let blob = ir.encode();
    let store = Local::default();
    let rt = Runtime::new(Box::new(store), Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())))
        .with_tags(Box::new(move || Arc::new(index.clone())));
    let s = Arc::new(Shared::new(rt));
    let (mut m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    m.map(INDEX, 0x100).map(ENTRIES, 0x100).map(DATA, 0x100);
    m.put_u32(TAG_INDEX_GLOBAL, INDEX)
        .put_u32(TAG_ENTRIES_GLOBAL, ENTRIES)
        .put_u32(INDEX + index_layout::TAG_COUNT, 8);
    let at = ENTRIES + 2 * 0x10;
    m.put_u32(at, u32::from_be_bytes(*b"scen"))
        .put_u32(at + 4, ROCK)
        .put_u32(at + 8, DATA)
        .put_f32(DATA + 4, 1.5)
        .put_u16(DATA + 0x90, (-3i16) as u16);
    let line = s.lock().start(&m, false);
    (m, s, line)
}

fn tick(s: &Arc<Shared>, m: &mut FakeGuest) {
    tick_at(s, m, TICK + 1);
}

/// A tick at game time `at`: the runtime runs a tick once, so a second
/// tick is a later time.
fn tick_at(s: &Arc<Shared>, m: &mut FakeGuest, at: u32) {
    m.put_u32(GAME_TIME + 8, at);
    s.lock().pre_tick(m);
    s.lock().post_tick(m, &[]);
}

fn global(s: &Arc<Shared>, name: &str) -> i32 {
    s.lock().running().unwrap().vm.global(name).unwrap()
}

#[test]
fn a_script_reads_a_tags_fields_from_the_maps_tag_in_guest_memory() {
    let (mut m, s, line) = start(READS, rock());
    assert!(s.lock().state().running, "{line}");
    tick(&s, &mut m);
    assert_eq!(global(&s, "radius"), fixed::ONE * 3 / 2);
    assert_eq!(global(&s, "message"), -3);
    // The tag is replaced in the guest: the read follows it, as every
    // machine that loads the map does.
    m.put_f32(DATA + 4, 4.0);
    tick_at(&s, &mut m, TICK + 2);
    assert_eq!(global(&s, "radius"), 4 * fixed::ONE);
}

#[test]
fn a_tag_the_guest_has_not_loaded_reads_zero() {
    let (mut m, s, line) = start(READS, rock());
    assert!(s.lock().state().running, "{line}");
    m.put_u32(ENTRIES + 2 * 0x10 + 4, 0xe002_0002);
    tick(&s, &mut m);
    assert_eq!(global(&s, "radius"), 0);
    assert_eq!(global(&s, "message"), 0);
}

#[test]
fn a_map_without_the_tag_blocks_the_game_by_name() {
    let other = TagIndex::new([("objects/props/tree".to_string(), FourCC(*b"scen"), 0xe001_0003)]);
    let (_, s, line) = start(READS, other);
    assert!(line.starts_with("rules: blocked, the game does not start: "), "{line}");
    let blocked = s.lock().blocked().unwrap().to_vec();
    // Each field once, with the reason.
    assert_eq!(blocked.len(), 2, "{blocked:?}");
    assert!(blocked[0].contains(PATH) && blocked[0].contains("object.bounding_radius"), "{blocked:?}");
    assert!(blocked[0].contains("the map has no such tag"), "{blocked:?}");
    assert!(blocked[1].contains("object.hud_message_index"), "{blocked:?}");
    assert!(s.lock().running().is_none());
}

#[test]
fn a_field_our_definitions_lack_or_a_float_read_as_an_int_blocks_the_game() {
    let missing = READS.replace("object.hud_message_index", "object.mana");
    let (_, s, _) = start(&missing, rock());
    let blocked = s.lock().blocked().unwrap().to_vec();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert!(blocked[0].contains("object.mana"), "{blocked:?}");

    let wrong = READS.replace("tag_fixed(\"objects/props/rock\", \"object.bounding_radius\")", "0.0")
        + "on init { message = tag_int(\"objects/props/rock\", \"object.bounding_radius\") }\n";
    let (_, s, _) = start(&wrong, rock());
    let blocked = s.lock().blocked().unwrap().to_vec();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert!(blocked[0].contains("it is a float: read it with tag_fixed"), "{blocked:?}");
}

#[test]
fn a_map_whose_tag_paths_could_not_be_read_blocks_a_script_that_reads_tags() {
    let (_, s, _) = start(READS, TagIndex::default());
    let blocked = s.lock().blocked().unwrap().to_vec();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert!(blocked[0].contains("tag paths could not be read"), "{blocked:?}");
    // A script with no tag read has nothing to check.
    let (_, s, line) = start(POINTS, TagIndex::default());
    assert!(s.lock().state().running, "{line}");
}

#[test]
fn a_film_skips_the_tag_check() {
    let ir = compile(READS);
    let blob = ir.encode();
    let rt = Runtime::new(Box::new(Local::default()), Box::new(move |_: [u8; 8]| Ok::<_, String>(blob.clone())))
        .with_film(true)
        .with_tags(Box::new(|| Arc::new(TagIndex::default())));
    let s = Arc::new(Shared::new(rt));
    let (m, _) = guest(block(prefix(&ir.id()), 0), SLAYER);
    let line = s.lock().start(&m, false);
    assert!(s.lock().state().running, "{line}");
}

#[test]
fn a_swapped_in_script_is_held_to_the_maps_tags() {
    // The running script has READS' slots, so a swap that fails on the
    // tags fails for that and not for a changed state shape.
    let (_, s, line) = start(READS, rock());
    assert!(s.lock().state().running, "{line}");
    let missing = compile(&READS.replace("props/rock", "props/gone"));
    let why = s.lock().swap(missing).unwrap_err();
    assert!(why.contains("the map does not have what the script needs"), "{why}");
    assert!(why.contains("props/gone"), "{why}");
    // One the map satisfies, of the same shape, swaps in.
    let same = READS.replace("message = tag_int(\"objects/props/rock\", \"object.hud_message_index\")", "message = 7");
    s.lock().swap(compile(&same)).unwrap();
}
