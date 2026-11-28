//! The observer resource walk on synthetic guest memory (no image): an
//! empty list returns success at once, the types the original ignores
//! walk without a call, a bitmap prediction in mode 2 calls the preload,
//! and a count too long to trust declines with the list untouched. The
//! lifted `16e5e0` is the oracle in `sim_seats_differential::predict_resources`.
use super::*;
use guest_layout::tags::{TAG_ENTRIES_GLOBAL, entry_layout};
use runtime::Guest;
use runtime::abi;
use runtime::guest::Permissions;

const RAM: u32 = 0x0080_0000;
const STACK: u32 = 0x007f_0000;
const LIST: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const TABLE: u32 = 0x0050_2000;
const TAG: u32 = 0x0050_3000;
/// The synthetic jump table: the image owns the real one, so the fixture
/// names the two types the tests call. Any other type below nine would
/// reach an unknown block and decline.
const BITMAP_TYPE: u16 = 8;
const MODEL_TYPE: u16 = 1;

fn walk(mode: u32, kinds: &[u16]) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0, RAM, Permissions::RW).unwrap();
    g.set_u32(STACK, 0x9000_0000).unwrap();
    g.set_u32(STACK + 4, LIST).unwrap();
    g.set_u32(LIST, kinds.len() as u32).unwrap();
    g.set_u32(LIST + 4, ENTRIES).unwrap();
    for (i, &kind) in kinds.iter().enumerate() {
        g.set_u16(ENTRIES + i as u32 * 8, kind).unwrap();
        g.set_u16(ENTRIES + i as u32 * 8 + 2, 5).unwrap();
        g.set_u32(ENTRIES + i as u32 * 8 + 4, 0x1234).unwrap();
    }
    g.set_u32(DISPATCH + BITMAP_TYPE as u32 * 4, BITMAP).unwrap();
    g.set_u32(DISPATCH + MODEL_TYPE as u32 * 4, MODEL).unwrap();
    g.set_u32(TAG_ENTRIES_GLOBAL, TABLE).unwrap();
    g.set_u32(TABLE + 0x1234 * entry_layout::STRIDE + entry_layout::ADDRESS, TAG).unwrap();
    g.set_u32(TAG + 0x48, 0x0060_0000).unwrap();
    let c = CpuAbi {
        edi: mode,
        esp: STACK,
        eflags: 0x202,
        mxcsr: 0x1f80,
        ..CpuAbi::default()
    };
    (c, g)
}

#[test]
fn an_empty_list_returns_success_at_once() {
    let (mut c, mut g) = walk(1, &[]);
    let t = run(&mut c, &mut g, ENTRY).unwrap().unwrap();
    assert_eq!(t.kind, abi::RETURN);
    assert_eq!((t.target_pc, c.eax as u8), (0x9000_0000, 1));
    assert_eq!(c.esp, STACK + 8);
}

#[test]
fn the_ignored_types_walk_without_a_call_and_keep_success() {
    for kinds in [&[9u16, 200][..], &[99, 0xffff, 9], &[BITMAP_TYPE]] {
        // Mode 1: the bitmap type is one of the direct successes too.
        let (mut c, mut g) = walk(1, kinds);
        let t = run(&mut c, &mut g, ENTRY).unwrap().unwrap();
        assert_eq!(t.kind, abi::RETURN, "{kinds:?}");
        assert_eq!(c.eax as u8, 1, "{kinds:?}");
        assert_eq!(c.esp, STACK + 8, "{kinds:?}");
        // Nothing but the success byte in the caller's frame changed.
        assert_eq!(g.u32(STACK).unwrap(), 0x9000_0000);
    }
}

#[test]
fn a_mode_two_bitmap_calls_the_preload_with_the_tag_record() {
    let (mut c, mut g) = walk(2, &[BITMAP_TYPE]);
    let t = run(&mut c, &mut g, ENTRY).unwrap().unwrap();
    assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, 0x0003_bcb0, BITMAP_PRELOAD_RETURN));
    // The bitmap group's index scaled onto the tag's own group list.
    assert_eq!(c.eax, 5 * 0x74 + 0x0060_0000);
    assert_eq!((c.ecx, c.edx), (TAG, TABLE));
}

#[test]
fn a_count_too_long_to_trust_declines_with_the_list_untouched() {
    let (mut c, mut g) = walk(2, &[9u16; 600]);
    let before = g.u32(LIST).unwrap();
    let cpu_before = c;
    assert_eq!(run(&mut c, &mut g, ENTRY).unwrap(), None);
    assert_eq!(g.u32(LIST).unwrap(), before);
    assert_eq!(c, cpu_before);
}

#[test]
fn a_model_prediction_calls_the_model_loader() {
    let (mut c, mut g) = walk(0, &[MODEL_TYPE]);
    let t = run(&mut c, &mut g, ENTRY).unwrap().unwrap();
    assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, 0x0016_ea60, MODEL_RETURN));
    assert_eq!(c.eax, 0x1234);
    // The tag was pushed as the cdecl argument, under the return site.
    assert_eq!(g.u32(c.esp + 4).unwrap(), 0x1234);
}
