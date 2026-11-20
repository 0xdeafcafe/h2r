//! Checks the registered addresses against the real retail XBE. Ignored by
//! default (the image is not in the repository). Read-only:
//!
//! `H2_XBE=/path/default.xbe cargo test -p xapi --test xbe_pins -- --ignored`
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register};
use runtime::{Guest, load_xbe};
use std::collections::BTreeSet;
use xapi::GuestLayout;
use xapi::register::{OVERRIDES, addr};

/// Every `ret` reachable from `entry` by direct jumps and fallthrough, with
/// its immediate. Calls fall through; indirect jumps end a path.
fn ret_sizes(mem: &Guest, entry: u32) -> BTreeSet<u16> {
    let mut rets = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut work = vec![entry];
    while let Some(start) = work.pop() {
        let mut ip = start;
        loop {
            if !seen.insert(ip) || seen.len() > 20_000 {
                break;
            }
            let mut bytes = [0u8; 16];
            if mem.read_bytes(ip, &mut bytes, 1).is_err() {
                break;
            }
            let mut d = Decoder::with_ip(32, &bytes, ip as u64, DecoderOptions::NONE);
            let i = d.decode();
            if i.is_invalid() {
                break;
            }
            let next = ip + i.len() as u32;
            match i.flow_control() {
                FlowControl::Return => {
                    if i.mnemonic() == Mnemonic::Ret {
                        rets.insert(if i.op_count() == 1 {
                            i.immediate16()
                        } else {
                            0
                        });
                    }
                    break;
                }
                FlowControl::UnconditionalBranch => {
                    work.push(i.near_branch_target() as u32);
                    break;
                }
                FlowControl::ConditionalBranch => {
                    work.push(i.near_branch_target() as u32);
                    ip = next;
                }
                FlowControl::IndirectBranch | FlowControl::Interrupt | FlowControl::Exception => {
                    break;
                }
                _ => ip = next,
            }
        }
    }
    rets
}

fn image() -> runtime::LoadedImage {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    load_xbe(&std::fs::read(path).unwrap()).unwrap()
}

/// Literal copy of `tests/routing.rs::VERIFIED_RET_SIZES`' addresses.
const PINNED_ABI: [(u32, u16); 32] = [
    (0x002d07f2, 8),
    (0x002d0915, 8),
    (0x002d1d3e, 0),
    (0x002d1d66, 4),
    (0x002d5813, 0),
    (0x002d5838, 16),
    (0x002d5b77, 4),
    (0x002d5c7c, 0),
    (0x002d5d0a, 0),
    (0x002d5f65, 4),
    (0x002d6004, 0),
    (0x002d6031, 0),
    (0x00330310, 0),
    (0x00330339, 0),
    (0x0033039a, 0),
    (0x00330df2, 24),
    (0x00331055, 8),
    (0x00331125, 8),
    (0x0033122f, 8),
    (0x00331276, 4),
    (0x00331385, 0),
    (0x003321bd, 0),
    (0x00408c57, 8),
    (0x00409884, 12),
    (0x004098c0, 8),
    (0x004098c5, 12),
    (0x00409932, 16),
    (0x00409988, 4),
    (0x00409994, 8),
    (0x00409b6c, 8),
    (0x00409bdf, 8),
    (0x0041136f, 20),
];

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn registered_addresses_have_their_pinned_ret_sizes_in_the_retail_image() {
    let image = image();
    let mem = image.runtime.memory();
    let mut bad = Vec::new();
    for o in OVERRIDES {
        let want = o.function.stack_words() as u16 * 4;
        let got = ret_sizes(&mem, o.addr);
        if got != BTreeSet::from([want]) {
            bad.push(format!(
                "{} {:#010x}: want ret {want}, found {got:?}",
                o.name, o.addr
            ));
        }
    }
    for (a, want) in PINNED_ABI {
        let got = ret_sizes(&mem, a);
        if got != BTreeSet::from([want]) {
            bad.push(format!("pinned {a:#010x}: want ret {want}, found {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The instruction at `va`, decoded. Checks below assert mnemonics and
/// operands, never copied instruction bytes.
fn insn(mem: &Guest, va: u32) -> Instruction {
    let mut bytes = [0u8; 16];
    mem.read_bytes(va, &mut bytes, 1).unwrap();
    let i = Decoder::with_ip(32, &bytes, va as u64, DecoderOptions::NONE).decode();
    assert!(!i.is_invalid(), "{va:#x}");
    i
}

/// `[disp]` with no base or index register.
fn absolute(i: &Instruction, op: u32) -> Option<u32> {
    (i.op_kind(op) == OpKind::Memory
        && i.memory_base() == Register::None
        && i.memory_index() == Register::None)
        .then(|| i.memory_displacement32())
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn thunk_gamepad_type_tls_index_and_cache_row_match_the_retail_image() {
    let image = image();
    let mem = image.runtime.memory();
    let at = |va| insn(&mem, va);

    // 004098c0: jmp XInitDevices.
    let i = at(addr::XINIT_DEVICES_THUNK);
    assert_eq!(i.mnemonic(), Mnemonic::Jmp);
    assert_eq!(i.near_branch_target() as u32, addr::XINIT_DEVICES);

    // 00124aae: push XDEVICE_TYPE_GAMEPAD; 00124abb: call XGetDeviceChanges.
    let i = at(0x0012_4aae);
    assert_eq!(i.mnemonic(), Mnemonic::Push);
    assert_eq!(i.immediate32(), addr::XDEVICE_TYPE_GAMEPAD);
    let i = at(0x0012_4abb);
    assert_eq!(i.mnemonic(), Mnemonic::Call);
    assert_eq!(i.near_branch_target() as u32, addr::XGET_DEVICE_CHANGES);

    // GetLastError: eax = [_tls_index]; ecx = fs:[4]; eax = [ecx+eax*4]; eax = [eax+8].
    let i = at(0x002d_1d50);
    assert_eq!(
        (i.mnemonic(), i.op0_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    assert_eq!(absolute(&i, 1), Some(addr::TLS_INDEX));
    let i = at(0x002d_1d55);
    assert_eq!(
        (i.mnemonic(), i.op0_register()),
        (Mnemonic::Mov, Register::ECX)
    );
    assert_eq!(i.memory_segment(), Register::FS);
    assert_eq!(absolute(&i, 1), Some(4));
    let i = at(0x002d_1d5c);
    assert_eq!(
        (i.memory_base(), i.memory_index(), i.memory_index_scale()),
        (Register::ECX, Register::EAX, 4)
    );
    let i = at(0x002d_1d5f);
    assert_eq!(
        (i.mnemonic(), i.memory_base()),
        (Mnemonic::Mov, Register::EAX)
    );
    assert_eq!(
        i.memory_displacement32(),
        GuestLayout::default().last_error_offset
    );

    // The cache global is a database ROW: stored at 0x2d5ac8, then used as
    // `row * 12` to write the partition index (esi - 3) into that row.
    let row = addr::CACHE_ROW;
    let i = at(0x002d_5ac8);
    assert_eq!(
        (i.mnemonic(), i.op1_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    assert_eq!(absolute(&i, 0), Some(row));
    let i = at(0x002d_5b19);
    assert_eq!(
        (i.mnemonic(), i.op0_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    assert_eq!(absolute(&i, 1), Some(row));
    let i = at(0x002d_5b22);
    assert_eq!(
        (i.mnemonic(), i.op0_register()),
        (Mnemonic::Add, Register::ESI)
    );
    assert_eq!(i.immediate(1) as u32 as i32, -3);
    let i = at(0x002d_5b28);
    assert_eq!(i.mnemonic(), Mnemonic::Lea);
    assert_eq!(
        (i.memory_base(), i.memory_index(), i.memory_index_scale()),
        (Register::EAX, Register::EAX, 2)
    );
    let i = at(0x002d_5b2b);
    assert_eq!(
        (i.mnemonic(), i.op1_register()),
        (Mnemonic::Mov, Register::ESI)
    );
    assert_eq!(
        (i.memory_base(), i.memory_index(), i.memory_index_scale()),
        (Register::EBP, Register::EAX, 4)
    );
    // 0x2d5d0a fails while the row is still -1.
    let i = at(0x002d_5d21);
    assert_eq!(i.mnemonic(), Mnemonic::Cmp);
    assert_eq!(absolute(&i, 0), Some(row));
    assert_eq!(i.immediate(1) as u32 as i32, -1);
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn cache_drive_globals_and_checks_match_the_retail_image() {
    use xapi::cache;
    let image = image();
    let mem = image.runtime.memory();
    let at = |va| insn(&mem, va);

    // 0x12003: game bootstrap calls the N: mount directly.
    let i = at(0x0001_2003);
    assert_eq!(i.mnemonic(), Mnemonic::Call);
    assert_eq!(i.near_branch_target() as u32, addr::XMOUNT_CACHE_DRIVE);

    // Title ID: eax = [certificate pointer]; eax = [eax+8].
    let i = at(0x002d_5d13);
    assert_eq!(absolute(&i, 1), Some(addr::XBE_CERTIFICATE_POINTER));
    let i = at(0x002d_5d18);
    assert_eq!(
        (i.memory_base(), i.memory_displacement32()),
        (Register::EAX, 8)
    );

    // Z: row == -1 fails with STATUS_UNSUCCESSFUL before anything else.
    let i = at(0x002d_5d2d);
    assert_eq!(i.mnemonic(), Mnemonic::Push);
    assert_eq!(i.immediate32(), 0xc000_0001);

    // Partition count: eax = [import]; esi = [eax]; clamp to 41 rows.
    let i = at(0x002d_5dab);
    assert_eq!(
        absolute(&i, 1),
        Some(addr::HAL_DISK_CACHE_PARTITION_COUNT_IMPORT)
    );
    let i = at(0x002d_5db0);
    assert_eq!(
        (i.op0_register(), i.memory_base()),
        (Register::ESI, Register::EAX)
    );
    let i = at(0x002d_5db2);
    assert_eq!(i.mnemonic(), Mnemonic::Cmp);
    assert_eq!(i.immediate(1) as usize, cache::CACHE_DB_ROWS);

    // Database checks: signature, marker, version.
    assert_eq!(
        at(0x002d_5dc0).immediate(1) as u32,
        cache::CACHE_DB_SIGNATURE
    );
    assert_eq!(at(0x002d_5dd0).immediate(1) as u32, cache::CACHE_DB_MARKER);
    assert_eq!(at(0x002d_5ddd).immediate(1) as u32, cache::CACHE_DB_VERSION);
    // A failed check is STATUS_INTERNAL_DB_CORRUPTION.
    assert_eq!(at(0x002d_5ea0).immediate(1) as u32, 0xc000_00e4);

    // N:'s row is stored once, and set to -1 on both failure paths.
    let i = at(0x002d_5e38);
    assert_eq!(
        (i.mnemonic(), i.op1_register()),
        (Mnemonic::Mov, Register::EAX)
    );
    assert_eq!(absolute(&i, 0), Some(addr::CACHE_DRIVE_ROW));
    for va in [0x002d_5f44, 0x002d_5f52] {
        let i = at(va);
        assert_eq!(i.mnemonic(), Mnemonic::Or, "{va:#x}");
        assert_eq!(absolute(&i, 0), Some(addr::CACHE_DRIVE_ROW), "{va:#x}");
        assert_eq!(i.immediate(1) as u32 as i32, -1, "{va:#x}");
    }
    // The format call and the \??\N: link creation through an import slot.
    let i = at(0x002d_5f07);
    assert_eq!(i.mnemonic(), Mnemonic::Call);
    assert_eq!(i.near_branch_target(), 0x002d_6c04);
    let i = at(0x002d_5f2a);
    assert_eq!(i.mnemonic(), Mnemonic::Call);
    assert!(absolute(&i, 0).is_some());
    // Plain ret: no argument bytes.
    let i = at(0x002d_5f64);
    assert_eq!((i.mnemonic(), i.op_count()), (Mnemonic::Ret, 0));
}
