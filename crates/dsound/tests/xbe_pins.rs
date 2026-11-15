//! Checks the registered addresses against the real retail XBE. Ignored by
//! default (the image is not in the repository). Read-only:
//!
//! `H2_XBE=/path/default.xbe cargo test -p dsound --test xbe_pins -- --ignored`
use dsound::Layout;
use dsound::overrides::ENTRIES;
use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic};
use runtime::{Guest, load_xbe};
use std::collections::BTreeSet;

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

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn registered_addresses_have_their_ret_sizes_in_the_retail_image() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mem = image.runtime.memory();
    let mut bad = Vec::new();
    for e in ENTRIES {
        // Inside the DSOUND section.
        if !(0x0037_9c60..0x0038_7374).contains(&e.va) {
            bad.push(format!("{} {:#010x}: outside DSOUND", e.name, e.va));
        }
        let want = e.argc as u16 * 4;
        let got = ret_sizes(&mem, e.va);
        if got != BTreeSet::from([want]) {
            bad.push(format!(
                "{} {:#010x}: want ret {want}, found {got:?}",
                e.name, e.va
            ));
        }
    }
    // The stream vtable the shim writes into every stream object holds the
    // seven CDirectSoundStream methods it overrides, in XMO order.
    let vtable = Layout::default().stream_vtable;
    for (slot, name) in [
        "CDirectSoundStream_AddRef",
        "CDirectSoundStream_Release",
        "CDirectSoundStream_GetInfo",
        "CDirectSoundStream_GetStatus",
        "CDirectSoundStream_Process",
        "CDirectSoundStream_Discontinuity",
        "CDirectSoundStream_Flush",
    ]
    .iter()
    .enumerate()
    {
        let word = mem.u32(vtable + 4 * slot as u32).unwrap();
        let entry = ENTRIES.iter().find(|e| e.name == *name).unwrap();
        if word != entry.va {
            bad.push(format!(
                "vtable slot {slot}: {word:#010x}, {name} is {:#010x}",
                entry.va
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn wma_codec_entries_have_their_ret_sizes_in_the_retail_image() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mem = image.runtime.memory();
    let mut bad = Vec::new();
    for (va, name, argc) in dsound::xwma::ENTRIES {
        let want = argc as u16 * 4;
        let got = ret_sizes(&mem, va);
        if got != BTreeSet::from([want]) {
            bad.push(format!("{name} {va:#010x}: want ret {want}, found {got:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
