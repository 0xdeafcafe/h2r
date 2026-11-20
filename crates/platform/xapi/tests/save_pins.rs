//! Checks the save-game override addresses against the real retail XBE by
//! decoding instructions. Ignored by default (the image is not in the
//! repository); read-only:
//!
//! `H2_XBE=/path/default.xbe cargo test -p xapi --test save_pins -- --ignored`
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind};
use runtime::{Guest, load_xbe};
use xapi::{
    register::{SAVE_OVERRIDES, addr},
    saves,
};

fn image() -> runtime::LoadedImage {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    load_xbe(&std::fs::read(path).unwrap()).unwrap()
}

/// The sign-extended immediate of a `push`.
fn imm(i: &Instruction) -> u32 {
    match i.op0_kind() {
        OpKind::Immediate8to32 => i.immediate8to32() as u32,
        _ => i.immediate32(),
    }
}

/// Decode `count` instructions from `start`.
fn decode(mem: &Guest, start: u32, count: usize) -> Vec<Instruction> {
    let mut bytes = vec![0u8; count * 15];
    mem.read_bytes(start, &mut bytes, 1).unwrap();
    Decoder::with_ip(32, &bytes, start as u64, DecoderOptions::NONE)
        .iter()
        .take(count)
        .collect()
}

/// Every `push imm` operand from `start` up to `end`, and the first `ret`'s
/// immediate.
fn pushes_and_ret(mem: &Guest, start: u32) -> (Vec<u32>, u16) {
    let end = match start {
        addr::XCREATE_SAVE_GAME => addr::XDELETE_SAVE_GAME,
        addr::XDELETE_SAVE_GAME => addr::XFIND_FIRST_SAVE_GAME,
        addr::XFIND_FIRST_SAVE_GAME => addr::XFIND_NEXT_SAVE_GAME,
        addr::XFIND_NEXT_SAVE_GAME => addr::XFIND_CLOSE,
        _ => start + 0x60,
    };
    let (mut pushes, mut ret) = (Vec::new(), None);
    for i in decode(mem, start, 600)
        .into_iter()
        .take_while(|i| i.ip32() < end)
    {
        if i.mnemonic() == Mnemonic::Push
            && matches!(i.op0_kind(), OpKind::Immediate32 | OpKind::Immediate8to32)
        {
            pushes.push(imm(&i));
        }
        if i.mnemonic() == Mnemonic::Ret {
            ret.get_or_insert(i.immediate16());
        }
    }
    (pushes, ret.expect("no ret"))
}

#[test]
#[ignore]
fn save_entry_points_have_the_pinned_cleanup() {
    let image = image();
    let mem = &image.runtime.memory();
    for (address, cleanup) in [
        (addr::XCREATE_SAVE_GAME, 24),
        (addr::XDELETE_SAVE_GAME, 8),
        (addr::XFIND_FIRST_SAVE_GAME, 8),
        (addr::XFIND_NEXT_SAVE_GAME, 8),
        (addr::XFIND_CLOSE, 4),
    ] {
        // A frame prologue, or XFindNext/XFindClose's direct register loads.
        let (_, ret) = pushes_and_ret(mem, address);
        assert_eq!(ret, cleanup, "{address:#x}");
    }
    for o in SAVE_OVERRIDES {
        assert_eq!(
            o.function.stack_words() as u16 * 4,
            pushes_and_ret(mem, o.addr).1,
            "{}",
            o.name
        );
    }
}

#[test]
#[ignore]
fn search_block_matches_the_original_allocation() {
    let image = image();
    let mem = &image.runtime.memory();
    // `push 24830000h; push 110h; call 00012280h` at 0x3311e0.
    let calls = decode(mem, 0x0033_11e0, 3);
    assert_eq!(calls[0].immediate32(), saves::SEARCH_HEAP_TAG);
    assert_eq!(calls[1].immediate32(), saves::SEARCH_SIZE);
    assert_eq!(calls[2].mnemonic(), Mnemonic::Call);
    assert_eq!(calls[2].near_branch_target() as u32, addr::GAME_ALLOCATE);
    // The block's signature word is stored as `mov dword ptr [esi], 53425645h`.
    let init = decode(mem, 0x0033_1201, 1);
    assert_eq!(init[0].mnemonic(), Mnemonic::Mov);
    assert_eq!(init[0].immediate32(), saves::SEARCH_SIGNATURE);
    // The root is copied to block + 0xc, with a 0x104-byte limit.
    let copy = decode(mem, 0x0033_11f5, 5);
    assert_eq!(copy[0].immediate32(), 0x104);
    assert_eq!(copy[2].memory_displacement32(), saves::SEARCH_OFFSET_ROOT);
}

#[test]
#[ignore]
fn create_writes_the_documented_metadata_and_hash() {
    let image = image();
    let mem = &image.runtime.memory();
    // `push 0FEFFh` (the byte-order mark) and `push 3Dh` ('=') feed the meta
    // format, and the name buffer holds at most 0x98 units.
    let (pushes, ret) = pushes_and_ret(mem, addr::XCREATE_SAVE_GAME);
    assert_eq!(ret, 24);
    assert!(pushes.contains(&0xfeff) && pushes.contains(&0x3d));
    assert!(pushes.contains(&0x98));
    // The hash modulus is pushed as 0x0000FFFF:0xFFFFFFC5 at 0x3309f1/0x3309f8.
    let hash = decode(mem, 0x0033_09f1, 3);
    assert_eq!(imm(&hash[0]), 0xffff);
    assert_eq!(imm(&hash[2]), 0xffff_ffc5);
    assert_eq!(
        (u64::from(imm(&hash[0])) << 32) | u64::from(imm(&hash[2])),
        saves::HASH_MODULUS
    );
    // 12 digits: the loop runs for shifts 0..=0x2c.
    let digits = decode(mem, 0x0033_0a36, 3);
    assert_eq!(digits[1].mnemonic(), Mnemonic::Dec);
    assert_eq!(digits[2].immediate32(), 0x2c);
}
