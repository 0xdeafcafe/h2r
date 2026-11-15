use super::*;

const BASE: u32 = 0x10000;
const TEXT: u32 = 0x11000;
const RDATA: u32 = 0x12000;

/// A minimal retail XBE: `.text` at 0x11000 holding `code`, and `.rdata` at
/// 0x12000 holding a kernel thunk table that imports KeBugCheck.
pub(crate) fn xbe_with(code: &[u8]) -> Vec<u8> {
    xbe_full(code, 0x100, &[])
}

/// As [`xbe_with`], with only `text_raw` bytes of `.text` in the file (the
/// rest of its 0x100 is the loader's zero fill), and `words` in `.rdata`
/// from 0x12010.
fn xbe_full(code: &[u8], text_raw: u32, words: &[u32]) -> Vec<u8> {
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
    put(&mut d, 0x210, text_raw);
    put(&mut d, 0x214, BASE + 0x280);
    d[0x280..0x286].copy_from_slice(b".text\0");
    // .rdata
    put(&mut d, 0x238, 0x02);
    put(&mut d, 0x23C, RDATA);
    put(&mut d, 0x240, 0x100);
    put(&mut d, 0x244, 0x1100);
    put(&mut d, 0x248, 0x100);
    put(&mut d, 0x24C, BASE + 0x290);
    d[0x290..0x297].copy_from_slice(b".rdata\0");
    // .text bytes, int3-filled
    d[0x1000..0x1100].fill(0xCC);
    d[0x1000..0x1000 + code.len()].copy_from_slice(code);
    // thunk table: KeBugCheck (95), then the terminator
    put(&mut d, 0x1100, 0x8000_0000 | 95);
    for (i, w) in words.iter().enumerate() {
        put(&mut d, 0x1110 + 4 * i, *w);
    }
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

#[test]
fn small_immediates_and_pointers_do_not_underflow() {
    // `mov eax, 0` / `push 2`: the immediates are candidate function starts
    // with fewer than three bytes before them, which used to panic
    // ("attempt to subtract with overflow") in debug builds.
    let data = xbe_with(&[0xB8, 0, 0, 0, 0, 0x6A, 0x02, 0xC3]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    assert!(a.functions.contains_key(&TEXT));
    assert!(!a.functions.contains_key(&0) && !a.functions.contains_key(&2));
}

#[test]
fn code_past_the_raw_data_is_zero_filled() {
    // `ret 4` whose last byte is past the file's bytes, as at the end of
    // two Bink sections: the loader maps it as 0, so it is `c2 04 00`. The
    // file byte there (int3 fill) must not be read.
    let data = xbe_full(&[0xC2, 0x04], 2, &[]);
    let x = Xbe::parse(&data).unwrap();
    let a = analyze(&x).unwrap();
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    assert_eq!(a.claim(TEXT + 2), Some(('i', Some(TEXT))));
    let text = &a.sections[0];
    assert_eq!((text.size, text.code, text.unknown), (0x100, 3, 0));
    let insn = a.function_cfg(&x, TEXT).unwrap();
    assert_eq!(insn.blocks[&TEXT].end, TEXT + 3);
    let mapped = image::mapped_bytes(&x, &x.sections[0]).unwrap();
    assert_eq!(mapped.len(), 0x100);
    assert_eq!(mapped[..2], [0xC2, 0x04]);
    assert!(mapped[2..].iter().all(|b| *b == 0), "the tail past the file's bytes is zero");
}

#[test]
fn a_data_pointer_just_past_a_jump_table_is_a_function() {
    // A switch with its table after its code, then a function at an
    // address nothing but alignment would accept, known only from `.rdata`.
    let code = pieces(&[
        (0x11000, &[0x83, 0xF8, 0x01]),                         // cmp eax, 1
        (0x11003, &[0x77, 0x0B]),                               // ja 0x11010
        (0x11005, &[0xFF, 0x24, 0x85, 0x14, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11014]
        (0x1100C, &[0xC3, 0xC3]),                               // the two cases
        (0x11010, &[0xC3]),                                     // default
        (0x11014, &[0x0C, 0x10, 0x01, 0x00, 0x0D, 0x10, 0x01, 0x00]),
        (0x1101C, &[0x33, 0xC0, 0xC3]),                         // xor eax, eax; ret
    ]);
    let data = xbe_full(&code, 0x100, &[0x1101C]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    assert_eq!(a.claim(0x1101B), Some(('d', Some(TEXT))));
    assert_eq!(a.functions.get(&0x1101C).map(|f| f.source), Some(Source::DataPointer));
    assert_eq!(a.pointers.entries, 1, "{:?}", a.pointers);
}

#[test]
fn a_data_pointer_into_a_function_body_is_not_a_function() {
    // The pointer lands mid-function, after a `call`, which is no boundary.
    let code = pieces(&[
        (0x11000, &[0xE8, 0x0B, 0, 0, 0]), // call 0x11010
        (0x11005, &[0x33, 0xC0, 0xC3]),    // xor eax, eax; ret
        (0x11010, &[0xC3]),
    ]);
    let data = xbe_full(&code, 0x100, &[0x11005, 0x11006]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    assert_eq!(a.functions.keys().copied().collect::<Vec<_>>(), [0x11000, 0x11010]);
    assert!(a.labels.is_empty());
    let p = a.pointers;
    assert_eq!((p.entries, p.interior, p.bodies), (0, 1, 1), "{p:?}");
}

#[test]
fn data_that_decodes_is_not_a_function() {
    // Pointed at from `.rdata`, all three decode cleanly to a `ret`: a
    // segment push, a far jump and a run of zero bytes are data.
    let code = pieces(&[
        (0x11000, &[0xC3]),
        (0x11010, &[0x06, 0xC3]),                                 // push es; ret
        (0x11020, &[0xEA, 0x27, 0x10, 0x01, 0x00, 0x08, 0x00]),   // jmp far 8:0x11027
        (0x11027, &[0xC3]),
        (0x11030, &[0x00, 0x00, 0xC3]),                           // add [eax], al; ret
        (0x11040, &[0x8C, 0xC8, 0xC3]),                           // mov eax, cs; ret (allowed)
    ]);
    let data = xbe_full(&code, 0x100, &[0x11010, 0x11020, 0x11030, 0x11040]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    assert_eq!(a.functions.keys().copied().collect::<Vec<_>>(), [0x11000, 0x11040]);
    assert_eq!(a.rejected.get(&(Source::DataPointer, ConflictKind::Implausible)), Some(&3), "{:?}", a.rejected);
}

#[test]
fn an_unguarded_table_skips_null_entries() {
    // `dec eax; jmp [eax*4+table]` with no range check: the values the
    // enum never takes are null entries, with cases after them. The table
    // ends at the int3 fill, and the trailing null is not a hole.
    let mut table = Vec::new();
    for t in [0x11008u32, 0x1100B, 0, 0, 0x1100E, 0] {
        table.extend_from_slice(&t.to_le_bytes());
    }
    let code = pieces(&[
        (0x11000, &[0x48]),                                     // dec eax
        (0x11001, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x11008, &[0x33, 0xC0, 0xC3]),                         // xor eax, eax; ret
        (0x1100B, &[0x40, 0x40, 0xC3]),
        (0x1100E, &[0x48, 0x48, 0xC3]),
        (0x11020, &table),
    ]);
    let (a, cfgs) = cfgs_of(code);
    let jt = &a.jump_tables[&0x11001];
    assert_eq!((jt.kind, jt.count), (SwitchKind::Unbounded, 5));
    assert_eq!(jt.targets, [0x11008, 0x1100B, 0x1100E]);
    assert_eq!(jt.holes, [2, 3]);
    assert_eq!(jt.cases().collect::<Vec<_>>(), [(0, 0x11008), (1, 0x1100B), (4, 0x1100E)]);
    assert_eq!(a.claim(0x11033), Some(('d', Some(TEXT))));
    assert_eq!(a.claim(0x11034).map(|c| c.0), Some('?'), "the trailing null is not claimed");
    assert!(!a.functions.contains_key(&0x1100E), "the case after the holes is not a function of its own");
    let indices: Vec<u32> = cfgs[&TEXT].blocks[&TEXT]
        .edges
        .iter()
        .filter_map(|e| match e.kind {
            cfg::EdgeKind::Switch { index, .. } => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(indices, [0, 1, 4]);
}

/// An unguarded `jmp [eax*4+0x11020]` at `TEXT` with `table` there and
/// three small cases at 0x11008, 0x1100B and 0x1100E.
fn unguarded_table(table: &[u32]) -> Analysis {
    let words: Vec<u8> = table.iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]),
        (0x11008, &[0x33, 0xC0, 0xC3]),
        (0x1100B, &[0x40, 0x40, 0xC3]),
        (0x1100E, &[0x48, 0x48, 0xC3]),
        (0x11020, &words),
    ]);
    cfgs_of(code).0
}

#[test]
fn an_unguarded_table_skips_at_most_four_nulls() {
    let a = unguarded_table(&[0x11008, 0, 0, 0, 0, 0x1100B]);
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (6, &vec![0x11008, 0x1100B], &vec![1, 2, 3, 4]));
    let a = unguarded_table(&[0x11008, 0, 0, 0, 0, 0, 0x1100B]);
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (1, &vec![0x11008], &vec![]));
    assert_eq!(a.claim(0x11024).map(|c| c.0), Some('?'), "five nulls end the table");
}

#[test]
fn a_null_does_not_lead_into_a_function_after_the_table() {
    // A one-entry table, a null, then a function known from `.rdata` whose
    // first word, 0x11068, happens to be a code address after the table.
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]),
        (0x11008, &[0xC3]),
        (0x11020, &0x11008u32.to_le_bytes()),
        (0x11024, &[0; 4]),
        (0x11028, &[0x68, 0x10, 0x01, 0x00, 0x00, 0x58, 0xC3]), // push 0x110; pop eax; ret
        (0x11068, &[0xC3]),
    ]);
    let data = xbe_full(&code, 0x100, &[0x11028]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.holes), (1, &vec![]));
    assert!(a.functions.contains_key(&0x11028));
    assert_eq!(a.claim(0x1102D), Some(('I', Some(0x11028))), "pop eax is code, not a table entry");
    assert_eq!(a.claim(0x11024).map(|c| c.0), Some('?'));
}

#[test]
fn a_null_does_not_join_two_tables() {
    // Two tables one null apart; the second's case is after both.
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]),
        (0x11008, &[0xC3]),
        (0x11020, &0x11008u32.to_le_bytes()),
        (0x11024, &[0; 4]),
        (0x11028, &0x11060u32.to_le_bytes()),
        (0x11060, &[0x33, 0xC0, 0xC3]),
    ]);
    let (a, _) = cfgs_of(code);
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (1, &vec![0x11008], &vec![]));
}

#[test]
fn a_null_does_not_lead_to_a_function_before_the_table() {
    // The word after the null is between the code and the table, where
    // cases go, but it is a function this one calls.
    let code = pieces(&[
        (0x11000, &[0xE8, 0x0B, 0x00, 0x00, 0x00]),             // call 0x11010
        (0x11005, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x1100C, &[0xC3]),
        (0x11010, &[0x33, 0xC0, 0xC3]),
        (0x11020, &0x1100Cu32.to_le_bytes()),
        (0x11024, &[0; 4]),
        (0x11028, &0x11010u32.to_le_bytes()),
    ]);
    let (a, _) = cfgs_of(code);
    let jt = &a.jump_tables[&0x11005];
    assert_eq!((jt.count, &jt.holes), (1, &vec![]));
    assert_eq!(a.functions.get(&0x11010).map(|f| f.source), Some(Source::Call));
}

#[test]
fn a_null_does_not_lead_into_a_function_that_looks_like_an_own_case() {
    // As above, but the function's first word, 0x11068, is a case of the
    // switch between its code and its table. Only `.rdata` says 0x11088
    // is a function, and that is enough to end the table at the null.
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x80, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11080]
        (0x11068, &[0xC3]),
        (0x11080, &0x11068u32.to_le_bytes()),
        (0x11084, &[0; 4]),
        (0x11088, &[0x68, 0x10, 0x01, 0x00, 0x00, 0x58, 0xC3]), // push 0x110; pop eax; ret
    ]);
    let data = xbe_full(&code, 0x100, &[0x11088]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (1, &vec![0x11068], &vec![]));
    assert!(a.functions.contains_key(&0x11088));
    assert_eq!(a.claim(0x11088), Some(('I', Some(0x11088))), "push is code, not a table entry");
}

#[test]
fn an_unguarded_table_keeps_cases_after_it_past_a_null() {
    // Cases after the table on both sides of the null: the case before
    // the null says this switch keeps its cases past its table.
    let words: Vec<u8> = [0x11060u32, 0, 0x11068].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x11020, &words),
        (0x11060, &[0x40, 0xC3]), // inc eax; ret
        (0x11068, &[0x48, 0xC3]), // dec eax; ret
    ]);
    let (a, cfgs) = cfgs_of(code);
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (3, &vec![0x11060, 0x11068], &vec![1]));
    assert!(cfgs[&TEXT].blocks.contains_key(&0x11068));
}

#[test]
fn a_null_leads_to_a_case_past_an_intervening_callee() {
    // The function this one calls sits between two of its cases.
    let words: Vec<u8> = [0x1100Cu32, 0, 0x11018].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xE8, 0x0B, 0x00, 0x00, 0x00]),             // call 0x11010
        (0x11005, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x1100C, &[0xC3]),
        (0x11010, &[0x33, 0xC0, 0xC3]), // xor eax, eax; ret
        (0x11018, &[0x40, 0xC3]),
        (0x11020, &words),
    ]);
    let (a, cfgs) = cfgs_of(code);
    let jt = &a.jump_tables[&0x11005];
    assert_eq!((jt.count, &jt.holes), (3, &vec![1]));
    assert!(cfgs[&TEXT].blocks.contains_key(&0x11018));
}

#[test]
fn a_null_leads_to_a_case_whose_address_is_taken() {
    // The function takes the address of its own case, which makes it a
    // label once its immediates are scanned.
    let words: Vec<u8> = [0x11010u32, 0, 0x11018].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0x68, 0x18, 0x10, 0x01, 0x00, 0x58]),       // push 0x11018; pop eax
        (0x11006, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x11010, &[0x40, 0xC3]),
        (0x11018, &[0x48, 0xC3]),
        (0x11020, &words),
    ]);
    let (a, cfgs) = cfgs_of(code);
    assert!(a.labels.contains(&0x11018));
    assert_eq!(a.jump_tables[&0x11006].count, 3);
    assert!(cfgs[&TEXT].interior_entries.contains(&0x11018));
}

#[test]
fn a_null_leads_to_a_case_shared_with_another_function() {
    // 0x11040 takes the address of 0x11053 and falls into it, so 0x11053
    // is that function's label before the switch, known only from
    // `.rdata`, is explored. The switch's case after the null jumps into
    // the same block.
    let words: Vec<u8> = [0x11030u32, 0, 0x11053].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xE8, 0x3B, 0x00, 0x00, 0x00, 0xC3]),       // call 0x11040; ret
        (0x11010, &[0xFF, 0x24, 0x85, 0x80, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11080]
        (0x11030, &[0xC3]),
        (0x11040, &[0xB8, 0x53, 0x10, 0x01, 0x00]), // mov eax, 0x11053
        (0x11045, &[0x90; 14]),
        (0x11053, &[0x40, 0xC3]),
        (0x11080, &words),
    ]);
    let data = xbe_full(&code, 0x100, &[0x11010]);
    let x = Xbe::parse(&data).unwrap();
    let a = analyze(&x).unwrap();
    assert!(a.labels.contains(&0x11053));
    assert_eq!(a.claim(0x11053), Some(('I', Some(0x11040))));
    let jt = &a.jump_tables[&0x11010];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (3, &vec![0x11030, 0x11053], &vec![1]));
    assert!(a.function_cfg(&x, 0x11010).unwrap().blocks.contains_key(&0x11053));
}

/// Discovery plus the CFG of every function, for `.text` holding `code`.
fn cfgs_of(code: Vec<u8>) -> (Analysis, BTreeMap<u32, cfg::FunctionCfg>) {
    let data = xbe_with(&code);
    let x = Xbe::parse(&data).unwrap();
    let a = analyze(&x).unwrap();
    let c = a.cfgs(&x);
    (a, c)
}

/// `.text` from `(va, bytes)` pieces over int3 fill.
fn pieces(pieces: &[(u32, &[u8])]) -> Vec<u8> {
    let mut c = vec![0xCCu8; 0x100];
    for (va, bytes) in pieces {
        let o = (va - TEXT) as usize;
        c[o..o + bytes.len()].copy_from_slice(bytes);
    }
    c
}

#[test]
fn cfg_calls_end_blocks_and_noreturn_calls_have_no_continuation() {
    use cfg::*;
    let (_, cfgs) = cfgs_of(program());
    let entry = &cfgs[&0x11000];
    assert!(entry.errors.is_empty(), "{:?}", entry.errors);
    assert_eq!(entry.blocks.keys().copied().collect::<Vec<_>>(), [0x11000, 0x11005]);
    assert_eq!(entry.blocks[&0x11000].edges, [Edge { kind: EdgeKind::CallContinuation, target: Some(0x11005), exits: false }]);
    assert_eq!(entry.blocks[&0x11005].edges, [Edge { kind: EdgeKind::NoReturnCall, target: Some(0x11010), exits: true }]);
    assert_eq!(entry.blocks[&0x11005].end, 0x1100A, "the dead ret after it is not part of the function");
    let targets: Vec<_> = entry.calls.iter().map(|c| (c.va, c.target, c.returns)).collect();
    assert_eq!(
        targets,
        [(0x11000, CallTarget::Direct(0x11020), true), (0x11005, CallTarget::Direct(0x11010), false)]
    );
    assert_eq!(entry.calls[0].continuation(), 0x11005);
}

#[test]
fn cfg_kernel_calls_carry_their_ordinal() {
    use cfg::*;
    let (_, cfgs) = cfgs_of(program());
    let f = &cfgs[&0x11010];
    assert_eq!(f.calls.len(), 1);
    assert_eq!(f.calls[0].target, CallTarget::Kernel { slot: RDATA, ordinal: 95 });
    assert!(!f.calls[0].returns);
    assert_eq!(f.blocks[&0x11010].edges, [Edge { kind: EdgeKind::KernelNoReturn(95), target: None, exits: true }]);
    assert_eq!(f.insn_count(), 1, "garbage after KeBugCheck is not decoded");
}

#[test]
fn cfg_switch_lists_every_table_entry() {
    use cfg::*;
    let (a, cfgs) = cfgs_of(program());
    let jt = &a.jump_tables[&0x11025];
    assert_eq!(
        *jt,
        JumpTable {
            site: 0x11025,
            table: 0x11030,
            kind: SwitchKind::Bounded,
            count: 3,
            bound: Some(3),
            index_table: None,
            targets: vec![0x11040, 0x11041, 0x11042],
            holes: vec![],
        }
    );
    let f = &cfgs[&0x11020];
    assert!(f.errors.is_empty(), "{:?}", f.errors);
    // cmp/ja | jmp table | three one-ret cases (0x11040 is also ja's target).
    assert_eq!(f.blocks.keys().copied().collect::<Vec<_>>(), [0x11020, 0x11025, 0x11040, 0x11041, 0x11042]);
    assert_eq!(
        f.blocks[&0x11020].edges,
        [Edge { kind: EdgeKind::Taken, target: Some(0x11040), exits: false }, Edge { kind: EdgeKind::NotTaken, target: Some(0x11025), exits: false }]
    );
    let sw: Vec<_> = f.blocks[&0x11025].edges.iter().map(|e| (e.kind, e.target.unwrap())).collect();
    let want: Vec<_> =
        (0..3).map(|i| (EdgeKind::Switch { site: 0x11025, table: 0x11030, index: i }, 0x11040 + i)).collect();
    assert_eq!(sw, want);
    assert!(f.blocks.values().filter(|b| b.start >= 0x11040).all(|b| b.edges[0].kind == EdgeKind::Return));
}

#[test]
fn cfg_branch_into_a_run_splits_it_and_tails_leave() {
    use cfg::*;
    let (_, cfgs) = cfgs_of(pieces(&[
        (0x11000, &[0x40]),                // inc eax
        (0x11001, &[0x48]),                // dec eax        <- loop head
        (0x11002, &[0x75, 0xFD]),          // jnz 0x11001
        (0x11004, &[0xE9, 0x37, 0, 0, 0]), // jmp 0x11040 (after int3 padding: a tail call)
        (0x11040, &[0xC3]),
    ]));
    let f = &cfgs[&0x11000];
    assert!(f.errors.is_empty(), "{:?}", f.errors);
    assert_eq!(f.blocks.keys().copied().collect::<Vec<_>>(), [0x11000, 0x11001, 0x11004]);
    assert_eq!(f.blocks[&0x11000].insns, [0x11000]);
    assert_eq!(f.blocks[&0x11000].edges, [Edge { kind: EdgeKind::Fallthrough, target: Some(0x11001), exits: false }]);
    assert_eq!(f.blocks[&0x11001].insns, [0x11001, 0x11002]);
    assert_eq!(f.blocks[&0x11004].edges, [Edge { kind: EdgeKind::Tail, target: Some(0x11040), exits: true }]);
    assert!(cfgs.contains_key(&0x11040));
    assert!(f.block_containing(0x11002).is_some_and(|b| b.start == 0x11001));
    assert!(f.block_containing(0x11003).is_none(), "mid-instruction");
}

#[test]
fn cfg_branches_and_fallthrough_into_another_entry_exit() {
    use cfg::*;
    let (_, cfgs) = cfgs_of(pieces(&[
        (0x11000, &[0xE8, 0x0B, 0, 0, 0]), // call 0x11010 (makes it a function)
        (0x11005, &[0x85, 0xC0]),          // test eax, eax
        (0x11007, &[0x74, 0x07]),          // jz 0x11010
        (0x11009, &[0x90; 7]),             // nops, falling into 0x11010
        (0x11010, &[0xC3]),                // ret
    ]));
    let f = &cfgs[&0x11000];
    assert!(f.errors.is_empty(), "{:?}", f.errors);
    assert_eq!(f.blocks.keys().copied().collect::<Vec<_>>(), [0x11000, 0x11005, 0x11009]);
    assert!(f.block_containing(0x11010).is_none(), "another function's entry is never absorbed");
    assert_eq!(
        f.blocks[&0x11005].edges,
        [
            Edge { kind: EdgeKind::Taken, target: Some(0x11010), exits: true },
            Edge { kind: EdgeKind::NotTaken, target: Some(0x11009), exits: false }
        ]
    );
    assert_eq!(f.blocks[&0x11009].edges, [Edge { kind: EdgeKind::Fallthrough, target: Some(0x11010), exits: true }]);
    assert_eq!(f.blocks[&0x11009].end, 0x11010);
}
