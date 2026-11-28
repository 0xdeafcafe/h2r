//! Notices against synthetic guest memory: a scenario, a shader and a
//! bitmap in an invented tag index. No game data.

use super::*;
use runtime::guest::Permissions;
use std::sync::{Arc, Mutex};

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const META: u32 = 0x1000_0000;

const SCNR: u32 = 0xe000_0000;
const SHAD: u32 = 0xe111_0001;
const BITM: u32 = 0xe222_0002;

const SHAD_AT: u32 = META + 0x1000;
const BITM_AT: u32 = META + 0x2000;

fn entry(g: &mut Guest, slot: u32, group: &[u8; 4], datum: u32, addr: u32, size: u32) {
    let e = ENTRIES + slot * 16;
    g.set_u32(e, FourCC(*group).as_u32()).unwrap();
    g.set_u32(e + 4, datum).unwrap();
    g.set_u32(e + 8, addr).unwrap();
    g.set_u32(e + 12, size).unwrap();
}

fn level() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW).unwrap();
    g.map_zeroed(META, 0x0001_0000, Permissions::RW).unwrap();
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(guest_layout::points::TAG_ENTRIES.base, ENTRIES).unwrap();
    g.set_u32(HEADER + index::HEADER_SCENARIO, SCNR).unwrap();
    g.set_u32(HEADER + index::HEADER_COUNT, 3).unwrap();
    entry(&mut g, 0, b"scnr", SCNR, META, 0x100);
    entry(&mut g, 1, b"shad", SHAD, SHAD_AT, 0x100);
    entry(&mut g, 2, b"bitm", BITM, BITM_AT, 0x100);
    g
}

/// An editor whose notices land in the returned list.
fn editor() -> (Editor, Arc<Mutex<Vec<TagWrite>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let ed = Editor::new().with_tag_writes(Box::new(move |_: &mut Guest, n: &TagWrite| sink.lock().unwrap().push(n.clone())));
    (ed, seen)
}

fn taken(seen: &Mutex<Vec<TagWrite>>) -> Vec<TagWrite> {
    std::mem::take(&mut *seen.lock().unwrap())
}

#[test]
fn a_field_batch_sends_one_notice_per_tag_and_a_revert_another() {
    let mut g = level();
    let (mut ed, seen) = editor();
    let writes = [
        json!({"at": SHAD_AT + 0x10, "value": "11223344"}),
        json!({"at": BITM_AT + 0x8, "value": "aa"}),
        json!({"at": SHAD_AT + 0x40, "value": "5566"}),
    ];
    ed.batch(&mut g, &writes, Some("two tags")).unwrap();
    let sent = taken(&seen);
    assert_eq!(sent.len(), 2, "one per tag: {sent:?}");
    assert_eq!(
        sent[0],
        TagWrite {
            group: FourCC(*b"shad"),
            tag: SHAD,
            address: SHAD_AT,
            size: 0x100,
            range: SHAD_AT + 0x10..SHAD_AT + 0x42,
            spans: vec![SHAD_AT + 0x10..SHAD_AT + 0x14, SHAD_AT + 0x40..SHAD_AT + 0x42],
            moved_blocks: Vec::new(),
            revert: false,
        }
    );
    assert_eq!(sent[1].group, FourCC(*b"bitm"));
    assert_eq!(sent[1].tag, BITM);
    assert_eq!(sent[1].spans, vec![BITM_AT + 0x8..BITM_AT + 0x9]);

    // The game holds the new bytes when the notice is read.
    let mut held = [0u8; 4];
    g.read_bytes(SHAD_AT + 0x10, &mut held, 1).unwrap();
    assert_eq!(held, [0x11, 0x22, 0x33, 0x44]);

    ed.revert(&mut g, None).unwrap();
    let back = taken(&seen);
    assert_eq!(back.len(), 2, "one per tag again: {back:?}");
    assert!(back.iter().all(|n| n.revert));
    let shad = back.iter().find(|n| n.tag == SHAD).unwrap();
    assert_eq!(shad.range, SHAD_AT + 0x10..SHAD_AT + 0x42);
    assert_eq!(shad.spans.len(), 2);
    assert_eq!(g.u32(SHAD_AT + 0x10).unwrap(), 0, "reverted");
}

#[test]
fn unchanged_bytes_and_refused_batches_send_nothing() {
    let mut g = level();
    let (mut ed, seen) = editor();
    // The bytes it already holds.
    ed.batch(&mut g, &[json!({"at": SHAD_AT, "value": "00000000"})], None).unwrap();
    assert!(taken(&seen).is_empty());
    // A mismatch writes nothing, so it says nothing.
    let refused = ed.batch(
        &mut g,
        &[json!({"at": SHAD_AT, "value": "01000000", "was": "02000000"})],
        None,
    );
    assert!(matches!(refused, Err(BatchError::Mismatch { .. })));
    assert!(taken(&seen).is_empty());
}

#[test]
fn a_set_sends_its_tag() {
    let mut g = level();
    let (mut ed, seen) = editor();
    let sel = TagSel { tag: format!("{BITM:#010x}"), group: None };
    ed.set(&mut g, &sel, "+0x20:u32", &json!(7)).unwrap();
    let sent = taken(&seen);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].tag, BITM);
    assert_eq!(sent[0].range, BITM_AT + 0x20..BITM_AT + 0x24);
    assert!(!sent[0].revert);
}
