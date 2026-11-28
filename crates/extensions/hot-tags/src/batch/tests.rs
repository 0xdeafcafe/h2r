//! Block moves against synthetic guest memory: a shader whose postprocess
//! colours block is grown into the edit arena (laid out by the
//! definitions' own offsets), and a scenario with an invented raw block.
//! The arena is the real window, its pages mapped in the synthetic guest.

use super::*;
use runtime::guest::Permissions;

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const META: u32 = 0x1000_0000;

const SCNR: u32 = 0xe000_0000;
const SHAD: u32 = 0xe111_0001;

const SHAD_AT: u32 = META + 0x1000;
const POST_AT: u32 = META + 0x3000;
const COLORS_AT: u32 = META + 0x4000;
/// An invented block at scnr+0x20: two elements of 0x30 bytes.
const RAW_AT: u32 = META + 0x8000;

fn shad() -> &'static tags::defs::StructDef {
    Defs::builtin().group(FourCC(*b"shad")).unwrap()
}

fn post() -> &'static tags::defs::StructDef {
    shad().child("postprocess").unwrap()
}

/// The colours block's element: its size and the `color` field's offset.
fn color() -> (u32, u32) {
    let c = post().child("colors").unwrap();
    (c.size, c.field("color").unwrap().offset)
}

/// The colours block's `(count, address)` pair.
fn colors_root() -> u32 {
    POST_AT + post().field("colors").unwrap().offset
}

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
    g.set_u32(HEADER + index::HEADER_COUNT, 2).unwrap();
    entry(&mut g, 0, b"scnr", SCNR, META, 0x100);
    entry(&mut g, 1, b"shad", SHAD, SHAD_AT, shad().size);
    let pp = shad().field("postprocess").unwrap().offset;
    g.set_u32(SHAD_AT + pp, 1).unwrap();
    g.set_u32(SHAD_AT + pp + 4, POST_AT).unwrap();
    g.set_u32(colors_root(), 2).unwrap();
    g.set_u32(colors_root() + 4, COLORS_AT).unwrap();
    let (stride, at) = color();
    g.set_u32(COLORS_AT + at, 0xff00_0001).unwrap();
    g.set_u32(COLORS_AT + stride + at, 0xff00_0002).unwrap();
    g.set_u32(META + 0x20, 2).unwrap();
    g.set_u32(META + 0x24, RAW_AT).unwrap();
    g.set_f32(RAW_AT + 0x30 + 0x8, 7.0).unwrap();
    g
}

fn sel(tag: &str) -> TagSel {
    TagSel { tag: tag.into(), group: None }
}

fn editor(g: &Guest) -> (Editor, arena::Shared) {
    let a = arena::shared();
    let mut ed = Editor::new().with_arena(a.clone());
    ed.at_epoch(g, 1, 1).unwrap();
    (ed, a)
}

/// The colours block's two elements and a third whose colour is `third`.
fn grown_colors(g: &Guest, third: u32) -> Vec<u8> {
    let (stride, at) = color();
    let mut bytes = read_bytes(g, COLORS_AT, 2 * stride).unwrap();
    let mut new = vec![0u8; stride as usize];
    new[at as usize..at as usize + 4].copy_from_slice(&third.to_le_bytes());
    bytes.extend(new);
    bytes
}

fn grow_colors(g: &Guest, third: u32, capacity: Option<u32>) -> RawBatch {
    RawBatch {
        moves: vec![Move {
            root: MoveRoot::Path { tag: sel("0xe1110001"), path: "postprocess[0].colors".into() },
            stride: color().0,
            count: 3,
            capacity,
            bytes: grown_colors(g, third),
            was: (2, COLORS_AT),
        }],
        ..RawBatch::default()
    }
}

fn pair_at(g: &Guest, root: u32) -> (u32, u32) {
    (g.u32(root).unwrap(), g.u32(root + 4).unwrap())
}

#[test]
fn a_move_then_a_path_read_resolves_through_the_new_address() {
    let mut g = level();
    let (mut ed, a) = editor(&g);
    let (stride, at) = color();
    let grown = grow_colors(&g, 0x1122_3344, Some(8));
    let r = ed.batch_with(&mut g, &[], &grown, Some("grow")).unwrap();
    let base = arena::WINDOW.start;
    assert_eq!(r["moves"][0]["address"], base, "{r}");
    assert_eq!(r["moves"][0]["capacity"], 8);
    assert_eq!(pair_at(&g, colors_root()), (3, base));
    // The path follows the repointed pair into the arena.
    let v = ed.get(&g, &sel("0xe1110001"), "postprocess[0].colors[2].color").unwrap();
    assert_eq!(v["addr"], json!(format!("{:#010x}", base + 2 * stride + at)), "{v}");
    assert_eq!(g.u32(base + 2 * stride + at).unwrap(), 0x1122_3344);
    let v = ed.get(&g, &sel("0xe1110001"), "postprocess[0].colors[0].color").unwrap();
    assert_eq!(v["addr"], json!(format!("{:#010x}", base + at)), "{v}");
    assert_eq!(g.u32(base + at).unwrap(), 0xff00_0001, "the old elements were copied");
    // The room past the count is zero, and the old block is left as it was.
    assert_eq!(read_bytes(&g, base + 3 * stride, 5 * stride).unwrap(), vec![0; 5 * stride as usize]);
    assert_eq!(g.u32(COLORS_AT + at).unwrap(), 0xff00_0001);
    // The arena books the extent to the shader, at the pair it serves.
    let held = arena::lock(&a);
    let e = held.extent(base).unwrap();
    assert_eq!((e.tag, e.root, e.stride, e.capacity), (SHAD, colors_root(), stride, 8));
    drop(held);
    // edit.arena reports it.
    let s = ed.arena(&mut g).unwrap();
    assert_eq!(s["moves"][0]["address"], base, "{s}");
    assert_eq!(s["base"], base);
    assert!(s["physical_free"].as_u64().unwrap() > 0, "{s}");
    assert!(s["used"].as_u64().unwrap() >= u64::from(8 * stride));
}

#[test]
fn an_at_write_inside_a_moved_block_is_admitted() {
    let mut g = level();
    let (mut ed, _a) = editor(&g);
    let (stride, at) = color();
    let grown = grow_colors(&g, 1, None);
    ed.batch_with(&mut g, &[], &grown, None).unwrap();
    let base = arena::WINDOW.start;
    let field = base + 2 * stride + at;
    let r = ed.batch(&mut g, &[json!({"at": field, "value": "efbeadde", "was": "01000000"})], None).unwrap();
    assert_eq!(r["changes"][0]["tag"], "0xe1110001", "{r}");
    assert_eq!(g.u32(field).unwrap(), 0xdead_beef);
    // edit.read reads it as tag bytes.
    assert_eq!(ed.read(&g, field, 4).unwrap()["holds_bytes"], "efbeadde");
    // Arena memory no move made is no tag's.
    let e = ed.batch(&mut g, &[json!({"at": base + 0x8000, "value": "00"})], None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    assert!(ed.read(&g, base + 0x8000, 1).is_err());
    // A BSP switch keeps the write: the extent still belongs to the shader.
    ed.at_epoch(&g, 2, 1).unwrap();
    let c = ed.changes(&g);
    assert_eq!(c["changes"].as_array().unwrap().len(), 1, "{c}");
    assert_eq!(c["moves"].as_array().unwrap().len(), 1, "{c}");
}

#[test]
fn revert_puts_the_pair_back_and_frees_the_extent() {
    let mut g = level();
    let (mut ed, a) = editor(&g);
    let (stride, at) = color();
    let grown = grow_colors(&g, 1, Some(4));
    let id = ed.batch_with(&mut g, &[], &grown, None).unwrap()["id"].as_u64().unwrap();
    let field = arena::WINDOW.start + 2 * stride + at;
    ed.batch(&mut g, &[json!({"at": field, "value": "02000000"})], None).unwrap();
    // Reverting everything undoes the write inside the block, then the move.
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["reverted"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["reverted"][1]["id"], id);
    assert_eq!(r["reverted"][1]["move"]["address"], arena::WINDOW.start);
    assert_eq!(pair_at(&g, colors_root()), (2, COLORS_AT));
    assert!(arena::lock(&a).is_empty());
    let e = ed.get(&g, &sel("0xe1110001"), "postprocess[0].colors[2].color").unwrap_err();
    assert!(e.message.contains("has 2 elements"), "{e}");
    assert_eq!(ed.changes(&g)["moves"], json!([]));

    // A move reverted on its own leaves a later write inside the block
    // nothing to restore: that write is dropped, not written into freed
    // arena memory.
    let grown = grow_colors(&g, 1, None);
    let id = ed.batch_with(&mut g, &[], &grown, None).unwrap()["id"].as_u64().unwrap();
    ed.batch(&mut g, &[json!({"at": field, "value": "03000000"})], None).unwrap();
    let r = ed.revert(&mut g, Some(id)).unwrap();
    assert_eq!(r["reverted"][0]["id"], id, "{r}");
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["reverted"], json!([]), "{r}");
    assert!(r["dropped"][0]["why"].as_str().unwrap().contains("no longer"), "{r}");

    // A root pointed elsewhere since is left alone, and its extent kept.
    let grown = grow_colors(&g, 1, None);
    let id = ed.batch_with(&mut g, &[], &grown, None).unwrap()["id"].as_u64().unwrap();
    g.set_u32(colors_root() + 4, COLORS_AT).unwrap();
    let r = ed.revert(&mut g, Some(id)).unwrap();
    assert!(r["dropped"][0]["why"].as_str().unwrap().contains("moved since"), "{r}");
    assert!(!arena::lock(&a).is_empty());
}

#[test]
fn a_move_is_checked_before_anything_is_written() {
    let mut g = level();
    let (mut ed, a) = editor(&g);
    let write = [json!({"at": META + 0x10, "value": "aa"})];
    // The root holds another pair than `was`.
    let mut batch = grow_colors(&g, 1, None);
    batch.moves[0].was = (2, COLORS_AT + 0x100);
    let e = ed.batch_with(&mut g, &write, &batch, None).unwrap_err();
    let BatchError::Mismatch { moves, .. } = e else { panic!("not a mismatch: {e:?}") };
    assert_eq!(moves[0]["holds"], json!({"count": 2, "address": COLORS_AT}));
    assert_eq!(g.u8(META + 0x10).unwrap(), 0);
    assert!(arena::lock(&a).is_empty());
    // The definition's element size is the only stride.
    let mut batch = grow_colors(&g, 1, None);
    batch.moves[0].stride += 4;
    assert!(matches!(ed.batch_with(&mut g, &[], &batch, None), Err(BatchError::Field(_))));
    // Bytes that are not count elements, a capacity under the count.
    let mut batch = grow_colors(&g, 1, None);
    batch.moves[0].bytes.pop();
    assert!(matches!(ed.batch_with(&mut g, &[], &batch, None), Err(BatchError::Field(_))));
    let grown = grow_colors(&g, 1, Some(2));
    assert!(matches!(ed.batch_with(&mut g, &[], &grown, None), Err(BatchError::Field(_))));
    // A value field is not a block.
    let mut batch = grow_colors(&g, 1, None);
    batch.moves[0].root = MoveRoot::Path { tag: sel("0xe1110001"), path: "postprocess[0].colors[0].color".into() };
    assert!(matches!(ed.batch_with(&mut g, &[], &batch, None), Err(BatchError::Field(_))));
    // A write over the root in the same batch.
    let over = [json!({"at": META + 0x24, "value": "00"})];
    let raw_move = RawBatch {
        moves: vec![Move {
            root: MoveRoot::At(META + 0x20),
            stride: 0x30,
            count: 2,
            capacity: Some(4),
            bytes: read_bytes(&g, RAW_AT, 0x60).unwrap(),
            was: (2, RAW_AT),
        }],
        ..RawBatch::default()
    };
    let e = ed.batch_with(&mut g, &over, &raw_move, None).unwrap_err();
    assert!(
        matches!(&e, BatchError::Field(Error { message, .. }) if message.contains("overlaps")),
        "{e:?}"
    );
    // No arena in this game.
    let mut bare = Editor::new();
    let grown = grow_colors(&g, 1, None);
    let e = bare.batch_with(&mut g, &[], &grown, None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Unavailable, .. })), "{e:?}");
    assert_eq!(pair_at(&g, colors_root()), (2, COLORS_AT));
    assert!(arena::lock(&a).is_empty());
}

#[test]
fn a_full_arena_refuses_and_writes_nothing() {
    let mut g = level();
    let (mut ed, a) = editor(&g);
    // Something else holds all but 0x100 bytes of the window.
    let len = arena::WINDOW.end - arena::WINDOW.start;
    arena::lock(&a)
        .alloc(&mut g, arena::Claim { tag: SCNR, root: META, stride: 1, capacity: len - 0x100 })
        .unwrap();
    let write = [json!({"at": META + 0x10, "value": "aa"})];
    // One element more than the free 0x100 bytes hold (the element is 4 bytes,
    // so 64 would fit exactly).
    let capacity = 0x100 / color().0 + 1;
    let grown = grow_colors(&g, 1, Some(capacity));
    let e = ed.batch_with(&mut g, &write, &grown, None).unwrap_err();
    let BatchError::Full { root, need, largest_free, .. } = e else { panic!("not full: {e:?}") };
    assert_eq!(root, colors_root());
    assert_eq!(need, (capacity * color().0).next_multiple_of(arena::ALIGN));
    assert_eq!(largest_free, 0x100);
    assert_eq!(g.u8(META + 0x10).unwrap(), 0, "the field write did not land");
    assert_eq!(pair_at(&g, colors_root()), (2, COLORS_AT));
    assert_eq!(arena::lock(&a).extents().count(), 1);
}

#[test]
fn a_root_by_address_moves_a_raw_block() {
    let mut g = level();
    let (mut ed, _a) = editor(&g);
    let mut bytes = read_bytes(&g, RAW_AT, 0x60).unwrap();
    let mut third = vec![0u8; 0x30];
    third[8..12].copy_from_slice(&9.0f32.to_le_bytes());
    bytes.extend(third);
    let batch = RawBatch {
        moves: vec![Move {
            root: MoveRoot::At(META + 0x20),
            stride: 0x30,
            count: 3,
            capacity: None,
            bytes,
            was: (2, RAW_AT),
        }],
        ..RawBatch::default()
    };
    let r = ed.batch_with(&mut g, &[], &batch, None).unwrap();
    assert_eq!(r["moves"][0]["tag"], "0xe0000000", "{r}");
    let s = sel("0xe0000000");
    let v = ed.get(&g, &s, "+0x20/0x30[2].+0x8:f32").unwrap();
    assert_eq!(v["value"], json!(9.0), "{v}");
    let v = ed.get(&g, &s, "+0x20/0x30[1].+0x8:f32").unwrap();
    assert_eq!(v["value"], json!(7.0), "{v}");
    // A pair outside every tag and moved block is no root.
    let mut stray = batch.clone();
    stray.moves[0].root = MoveRoot::At(0x0040_0000);
    assert!(matches!(ed.batch_with(&mut g, &[], &stray, None), Err(BatchError::Field(_))));
}

#[test]
fn a_new_map_releases_the_arena() {
    let mut g = level();
    let (mut ed, a) = editor(&g);
    let grown = grow_colors(&g, 1, None);
    ed.batch_with(&mut g, &[], &grown, None).unwrap();
    ed.bind_arena(&mut g);
    assert!(!arena::lock(&a).is_empty(), "the same map keeps it");
    ed.at_epoch(&g, 5, 5).unwrap();
    assert_eq!(ed.changes(&g)["moves"], json!([]));
    ed.bind_arena(&mut g);
    let held = arena::lock(&a);
    assert!(held.is_empty());
    assert_eq!(held.mapped(), 0);
    drop(held);
    assert!(g.u32(arena::WINDOW.start).is_err(), "its pages are unmapped");
}

#[test]
fn a_move_and_its_revert_report_the_displaced_block() {
    use std::sync::{Arc, Mutex};

    let mut g = level();
    let (mut ed, _) = editor(&g);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    ed = ed.with_tag_writes(Box::new(move |_, notice| {
        sink.lock().unwrap().push(notice.clone());
    }));

    let batch = grow_colors(&g, 7, None);
    let event = ed.batch_with(&mut g, &[], &batch, None).unwrap();
    let id = event["id"].as_u64().unwrap();
    ed.revert(&mut g, Some(id)).unwrap();

    let notices = seen.lock().unwrap();
    assert_eq!(notices.len(), 2);
    for (notice, (revert, previous)) in notices.iter().zip([
        (false, (2, COLORS_AT)),
        (true, (3, arena::WINDOW.start)),
    ]) {
        assert_eq!(notice.tag, SHAD);
        assert_eq!(notice.revert, revert);
        assert_eq!(notice.range, colors_root()..colors_root() + 8);
        // A root move still doesn't send the pair to the metadata overlay.
        assert!(notice.spans.is_empty());
        assert_eq!(notice.moved_blocks, [BlockRepoint { root: colors_root(), previous }]);
    }
}
