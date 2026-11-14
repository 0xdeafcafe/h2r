use super::*;

const BASE: u32 = 0x10000;
const TEXT: u32 = 0x11000;
const RDATA: u32 = 0x12000;

/// A minimal retail XBE: `.text` at 0x11000 holding `code`, and `.rdata` at
/// 0x12000 holding a kernel thunk table that imports KeBugCheck.
fn xbe_with(code: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x1200];
    let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    d[..4].copy_from_slice(b"XBEH");
    put(&mut d, 0x104, BASE);
    put(&mut d, 0x108, 0x1000);
    put(&mut d, 0x10C, 0x3000);
    put(&mut d, 0x118, BASE + 0x400);
    put(&mut d, 0x11C, 2);
    put(&mut d, 0x120, BASE + 0x200);
    put(&mut d, 0x128, TEXT ^ 0xA8FC_57AB);
    put(&mut d, 0x158, RDATA ^ 0x5B6D_40B6);
    // .text
    put(&mut d, 0x200, 0x06);
    put(&mut d, 0x204, TEXT);
    put(&mut d, 0x208, 0x100);
    put(&mut d, 0x20C, 0x1000);
    put(&mut d, 0x210, 0x100);
    put(&mut d, 0x214, BASE + 0x280);
    d[0x280..0x286].copy_from_slice(b".text\0");
    // .rdata
    put(&mut d, 0x238, 0x02);
    put(&mut d, 0x23C, RDATA);
    put(&mut d, 0x240, 0x10);
    put(&mut d, 0x244, 0x1100);
    put(&mut d, 0x248, 0x10);
    put(&mut d, 0x24C, BASE + 0x290);
    d[0x290..0x297].copy_from_slice(b".rdata\0");
    // .text bytes, int3-filled
    d[0x1000..0x1100].fill(0xCC);
    d[0x1000..0x1000 + code.len()].copy_from_slice(code);
    // thunk table: KeBugCheck (95), then the terminator
    put(&mut d, 0x1100, 0x8000_0000 | 95);
    d
}

/// Entry calls a guarded 3-way switch, then a function that calls
/// KeBugCheck and is followed by garbage.
fn program() -> Vec<u8> {
    let mut c = vec![0xCCu8; 0x60];
    let at = |c: &mut Vec<u8>, va: u32, bytes: &[u8]| {
        let o = (va - TEXT) as usize;
        c[o..o + bytes.len()].copy_from_slice(bytes);
    };
    at(&mut c, 0x11000, &[0xE8, 0x1B, 0, 0, 0]); // call 0x11020 (switch)
    at(&mut c, 0x11005, &[0xE8, 0x06, 0, 0, 0]); // call 0x11010 (never returns)
    at(&mut c, 0x1100A, &[0xC3]); // unreachable
    at(&mut c, 0x11010, &[0xFF, 0x15, 0x00, 0x20, 0x01, 0x00]); // call [KeBugCheck]
    at(&mut c, 0x11016, &[0xFF, 0xFF]); // garbage after the noreturn call
    at(&mut c, 0x11020, &[0x83, 0xF8, 0x02]); // cmp eax, 2
    at(&mut c, 0x11023, &[0x77, 0x1B]); // ja 0x11040
    at(&mut c, 0x11025, &[0xFF, 0x24, 0x85, 0x30, 0x10, 0x01, 0x00]); // jmp [eax*4+0x11030]
    for (i, t) in [0x11040u32, 0x11041, 0x11042].iter().enumerate() {
        at(&mut c, 0x11030 + i as u32 * 4, &t.to_le_bytes());
    }
    at(&mut c, 0x1103C, &0x11050u32.to_le_bytes()); // looks like a 4th entry, isn't
    at(&mut c, 0x11040, &[0xC3, 0xC3, 0xC3]);
    at(&mut c, 0x11050, &[0xFF, 0xFF]); // a conflict if anything decodes it
    c
}

fn analysed() -> Analysis {
    let data = xbe_with(&program());
    let x = Xbe::parse(&data).unwrap();
    analyze(&x).unwrap()
}

#[test]
fn calls_that_never_return_end_the_block() {
    let a = analysed();
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    assert!(!a.functions[&0x11010].returns);
    // The caller must not fall through the call. (The gap scan may still
    // pick the stray `ret` up as its own function; that's a separate pass.)
    let after_call = a.claim(0x1100A).unwrap();
    assert_ne!(after_call.1, Some(0x11000), "entry fell through a call that never returns");
    assert_eq!(a.claim(0x11016).unwrap().0, '?', "garbage after KeBugCheck was decoded");
    assert_eq!(a.kernel_calls.get(&95), Some(&1));
}

#[test]
fn guarded_switch_reads_exactly_its_entries() {
    let a = analysed();
    assert_eq!(a.switches.get(&0x11025), Some(&SwitchKind::Bounded));
    assert!(a.functions[&0x11020].returns);
    for slot in 0x11030..0x1103C {
        assert_eq!(a.claim(slot).unwrap().0, 'd');
    }
    assert_eq!(a.claim(0x1103C).unwrap().0, '?', "read past the guard's bound");
    assert_eq!(a.claim(0x11042).unwrap().0, 'I');
}

#[test]
fn alignment_nops_are_padding() {
    assert_eq!(nop_len(&[0x8D, 0xA4, 0x24, 0, 0, 0, 0], 0), 7); // lea esp, [esp+0]
    assert_eq!(nop_len(&[0x8D, 0x9B, 0, 0, 0, 0], 0), 6); // lea ebx, [ebx+0]
    assert_eq!(nop_len(&[0x8D, 0x49, 0x00], 0), 3); // lea ecx, [ecx+0]
    assert_eq!(nop_len(&[0x8B, 0xFF], 0), 0, "mov edi, edi starts hot-patchable functions");
    assert_eq!(nop_len(&[0x8D, 0x48, 0x04], 0), 0, "lea ecx, [eax+4] does real work");
    let run = [0xCC, 0xCC, 0x8D, 0x64, 0x24, 0x00, 0x90, 0x55];
    assert_eq!(padding_len(&run, 0), 7);
}

/// Analyse `.text` built from `(va, bytes)` pieces over int3 fill.
fn analyse(pieces: &[(u32, &[u8])]) -> Analysis {
    let mut c = vec![0xCCu8; 0x100];
    for (va, bytes) in pieces {
        let o = (va - TEXT) as usize;
        c[o..o + bytes.len()].copy_from_slice(bytes);
    }
    let data = xbe_with(&c);
    let x = Xbe::parse(&data).unwrap();
    analyze(&x).unwrap()
}

const JMP_TABLE_11020: [u8; 7] = [0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]; // jmp [eax*4+0x11020]
const CASES: [u8; 16] = [
    0x40, 0x10, 0x01, 0x00, 0x41, 0x10, 0x01, 0x00, 0x42, 0x10, 0x01, 0x00, 0x43, 0x10, 0x01, 0x00,
];

#[test]
fn a_bound_is_forgotten_when_the_index_is_rewritten() {
    let a = analyse(&[
        (0x11000, &[0x83, 0xF8, 0x01]),             // cmp eax, 1
        (0x11003, &[0x77, 0x13]),                   // ja 0x11018
        (0x11005, &[0xB8, 0x03, 0, 0, 0]),          // mov eax, 3
        (0x1100A, &JMP_TABLE_11020),
        (0x11018, &[0xC3]),
        (0x11020, &CASES),
        (0x11040, &[0xC3, 0xC3, 0xC3, 0xC3]),
    ]);
    assert_eq!(a.switches.get(&0x1100A), Some(&SwitchKind::Unbounded));
    assert_eq!(a.claim(0x11043).unwrap().0, 'I', "case 3 is reachable");
}

#[test]
fn a_bound_is_forgotten_when_flags_are_reset() {
    let a = analyse(&[
        (0x11000, &[0x83, 0xF8, 0x01]), // cmp eax, 1
        (0x11003, &[0x85, 0xC9]),       // test ecx, ecx
        (0x11005, &[0x77, 0x11]),       // ja 0x11018: tests ecx, not eax
        (0x11007, &JMP_TABLE_11020),
        (0x11018, &[0xC3]),
        (0x11020, &CASES),
        (0x11040, &[0xC3, 0xC3, 0xC3, 0xC3]),
    ]);
    assert_eq!(a.switches.get(&0x11007), Some(&SwitchKind::Unbounded));
}

#[test]
fn jbe_falls_through_on_the_unbounded_side() {
    let a = analyse(&[
        (0x11000, &[0x83, 0xF8, 0x01]), // cmp eax, 1
        (0x11003, &[0x76, 0x13]),       // jbe 0x11018: fall-through has eax > 1
        (0x11005, &JMP_TABLE_11020),
        (0x11018, &[0xC3]),
        (0x11020, &CASES),
        (0x11040, &[0xC3, 0xC3, 0xC3, 0xC3]),
    ]);
    assert_ne!(a.switches.get(&0x11005), Some(&SwitchKind::Bounded));
    assert_eq!(a.claim(0x11043).unwrap().0, 'I');
}

#[test]
fn mutual_tail_calls_that_never_return_stay_noreturn() {
    let a = analyse(&[
        (0x11000, &[0xE8, 0x1B, 0, 0, 0]), // call 0x11020
        (0x11005, &[0xC3]),                // dead
        (0x11020, &[0xEB, 0x1E]),          // jmp 0x11040 (tail)
        (0x11040, &[0xEB, 0xDE]),          // jmp 0x11020 (tail)
    ]);
    assert!(!a.functions[&0x11020].returns);
    assert!(!a.functions[&0x11040].returns);
    assert_ne!(a.claim(0x11005).unwrap().1, Some(0x11000), "entry resumed after a call into an endless loop");
}

#[test]
fn self_recursion_with_no_way_out_never_returns() {
    let a = analyse(&[
        (0x11000, &[0xE8, 0x1B, 0, 0, 0]),       // call 0x11020
        (0x11005, &[0xC3]),                      // dead
        (0x11020, &[0xE8, 0xFB, 0xFF, 0xFF, 0xFF]), // call 0x11020
        (0x11025, &[0xFF, 0xFF]),                // garbage
    ]);
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    assert!(!a.functions[&0x11020].returns);
    assert_ne!(a.claim(0x11005).unwrap().1, Some(0x11000));
    assert_eq!(a.claim(0x11025).unwrap().0, '?');
}

#[test]
fn an_unreadable_switch_is_no_proof_of_noreturn() {
    let a = analyse(&[
        (0x11000, &[0xE8, 0x1B, 0, 0, 0]),                    // call 0x11020
        (0x11005, &[0xC3]),
        (0x11020, &[0xFF, 0x24, 0x85, 0x30, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11030]
        // 0x11030: int3 fill, so the table's first entry isn't code
    ]);
    assert!(a.functions[&0x11020].returns);
    assert_eq!(a.claim(0x11005).unwrap(), ('I', Some(0x11000)));
}

#[test]
fn a_bad_opcode_blames_nobody_else() {
    let a = analyse(&[
        (0x11000, &[0xE8, 0x1B, 0, 0, 0]),                   // call 0x11020
        (0x11005, &[0xE8, 0x1E, 0, 0, 0]),                   // call 0x11028
        (0x1100A, &[0xC3]),
        (0x11020, &[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0xFF, 0xFF]), // runs into garbage
        (0x11028, &[0xC3]),
    ]);
    let c = a.conflicts.iter().find(|c| c.kind == ConflictKind::InvalidOpcode).expect("conflict");
    assert_eq!(c.claimed_by, None, "{c:?}");
}
