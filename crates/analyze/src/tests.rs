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

/// An unguarded `jmp [eax*4+0x11020]` at `TEXT` with the table
/// `[0x11008, 0, 0x1100B]` and its two cases, and `words` in `.rdata`.
fn table_with_rdata(words: &[u32]) -> (Analysis, cfg::FunctionCfg) {
    let table: Vec<u8> = [0x11008u32, 0, 0x1100B].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x11008, &[0x40, 0xC3]),                               // inc eax; ret
        (0x1100B, &[0x48, 0xC3]),                               // dec eax; ret
        (0x11020, &table),
    ]);
    let data = xbe_full(&code, 0x100, words);
    let x = Xbe::parse(&data).unwrap();
    let a = analyze(&x).unwrap();
    let cfg = a.function_cfg(&x, TEXT).unwrap();
    (a, cfg)
}

#[test]
fn a_data_pointer_to_a_table_slot_does_not_end_the_table() {
    // `.rdata` holds the address of the table's third entry, which reads
    // as no function, so it says nothing about where the table ends.
    let (a, cfg) = table_with_rdata(&[0x11028]);
    let jt = &a.jump_tables[&TEXT];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (3, &vec![0x11008, 0x1100B], &vec![1]));
    assert!(cfg.blocks.contains_key(&0x1100B));
    assert!(!a.functions.contains_key(&0x11028));
}

#[test]
fn a_data_pointer_to_a_case_does_not_end_the_table() {
    let (a, cfg) = table_with_rdata(&[0x1100B]);
    assert_eq!((a.jump_tables[&TEXT].count, &a.jump_tables[&TEXT].holes), (3, &vec![1]));
    assert!(cfg.blocks.contains_key(&0x1100B));
}

#[test]
fn a_null_does_not_join_a_known_table_with_cases_after_it() {
    // The callee's switch is read first; its table follows the caller's
    // one-entry table and null, and both tables' cases are after them.
    let code = pieces(&[
        (0x11000, &[0xE8, 0x3B, 0x00, 0x00, 0x00]),             // call 0x11040
        (0x11005, &[0xFF, 0x24, 0x85, 0x80, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11080]
        (0x11040, &[0xFF, 0x24, 0x85, 0x88, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11088]
        (0x11080, &0x110B0u32.to_le_bytes()),
        (0x11084, &[0; 4]),
        (0x11088, &0x110C0u32.to_le_bytes()),
        (0x110B0, &[0x40, 0xC3]),
        (0x110C0, &[0x48, 0xC3]),
    ]);
    let (a, cfgs) = cfgs_of(code);
    assert_eq!(a.jump_tables[&0x11040].count, 1);
    let jt = &a.jump_tables[&0x11005];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (1, &vec![0x110B0], &vec![]));
    assert!(!cfgs[&TEXT].blocks.contains_key(&0x110C0));
    assert_eq!(a.claim(0x11088), Some(('d', Some(0x11040))), "the callee keeps its table");
}

#[test]
fn a_null_does_not_join_a_known_table_whose_case_is_a_label() {
    // 0x11040 takes the address of its own case 0x11053 and switches to
    // it from the table at 0x11088. The switch at 0x11010, known only from
    // `.rdata`, has a one-entry table and a null just before that one.
    let code = pieces(&[
        (0x11000, &[0xE8, 0x3B, 0x00, 0x00, 0x00, 0xC3]),       // call 0x11040; ret
        (0x11010, &[0xFF, 0x24, 0x85, 0x80, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11080]
        (0x11030, &[0xC3]),
        (0x11040, &[0xB8, 0x53, 0x10, 0x01, 0x00]),             // mov eax, 0x11053
        (0x11045, &[0xFF, 0x24, 0x85, 0x88, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11088]
        (0x11053, &[0x40, 0xC3]),
        (0x11080, &0x11030u32.to_le_bytes()),
        (0x11084, &[0; 4]),
        (0x11088, &0x11053u32.to_le_bytes()),
    ]);
    let data = xbe_full(&code, 0x100, &[0x11010]);
    let a = analyze(&Xbe::parse(&data).unwrap()).unwrap();
    assert!(a.labels.contains(&0x11053));
    assert_eq!(a.jump_tables[&0x11045].count, 1);
    let jt = &a.jump_tables[&0x11010];
    assert_eq!((jt.count, &jt.targets, &jt.holes), (1, &vec![0x11030], &vec![]));
}

#[test]
fn a_null_does_not_lead_into_the_middle_of_an_instruction() {
    // The word after the null is inside the callee's `mov eax, imm32`.
    let words: Vec<u8> = [0x1100Cu32, 0, 0x11011].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xE8, 0x0B, 0x00, 0x00, 0x00]),             // call 0x11010
        (0x11005, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x1100C, &[0xC3]),
        (0x11010, &[0xB8, 0x40, 0x40, 0x40, 0x40, 0xC3]), // mov eax, 0x40404040; ret
        (0x11020, &words),
    ]);
    let (a, _) = cfgs_of(code);
    assert_eq!(a.jump_tables[&0x11005].count, 1);
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

#[test]
fn a_null_does_not_lead_into_known_data() {
    // The word after the null is another switch's table.
    let words: Vec<u8> = [0x1100Cu32, 0, 0x11080].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xE8, 0x3B, 0x00, 0x00, 0x00]),             // call 0x11040
        (0x11005, &[0xFF, 0x24, 0x85, 0x90, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11090]
        (0x1100C, &[0xC3]),
        (0x11040, &[0xFF, 0x24, 0x85, 0x80, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11080]
        (0x11060, &[0xC3]),
        (0x11080, &0x11060u32.to_le_bytes()),
        (0x11090, &words),
    ]);
    let (a, _) = cfgs_of(code);
    assert_eq!(a.jump_tables[&0x11005].count, 1);
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

/// `prefix` at `TEXT`, then `jmp [eax*4+0x11020]`. The table holds four
/// cases at 0x11040..0x11043, then a word that looks like a fifth: it
/// points at bytes that don't decode.
fn switch_after(prefix: &[u8]) -> Analysis {
    let jmp_at = TEXT + prefix.len() as u32;
    let code = pieces(&[
        (TEXT, prefix),
        (jmp_at, &JMP_TABLE_11020),
        (0x11020, &CASES),
        (0x11030, &0x11050u32.to_le_bytes()),
        (0x11040, &[0xC3, 0xC3, 0xC3, 0xC3]),
        (0x11050, &[0xFF, 0xFF]),
    ]);
    cfgs_of(code).0
}

/// [`switch_after`]'s table, checked to be sized to its four cases.
fn assert_four_cases(a: &Analysis, site: u32) {
    let jt = &a.jump_tables[&site];
    assert_eq!((jt.kind, jt.count, jt.bound), (SwitchKind::Bounded, 4, Some(4)));
    assert_eq!(jt.targets, [0x11040, 0x11041, 0x11042, 0x11043]);
    assert_eq!(a.claim(0x11030).unwrap().0, '?', "the word after the table is not read");
    assert_eq!(a.claim(0x11050).unwrap().0, '?');
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

#[test]
fn an_and_mask_bounds_a_switch() {
    let a = switch_after(&[0x83, 0xE0, 0x03]); // and eax, 3
    assert_four_cases(&a, 0x11003);
}

#[test]
fn an_and_mask_bounds_a_switch_through_a_copy() {
    // and ecx, 3; cmp edx, 8 (flags are no matter); mov eax, ecx
    let a = switch_after(&[0x83, 0xE1, 0x03, 0x83, 0xFA, 0x08, 0x8B, 0xC1]);
    assert_four_cases(&a, 0x11008);
    // mov eax, 3; and eax, ecx: `eax` holds at most the constant.
    let a = switch_after(&[0xB8, 0x03, 0, 0, 0, 0x23, 0xC1]);
    assert_four_cases(&a, 0x11007);
}

#[test]
fn a_mask_is_forgotten_when_the_index_is_rewritten() {
    // and ecx, 3; mov eax, ecx; inc eax: the copy no longer holds the mask.
    let a = switch_after(&[0x83, 0xE1, 0x03, 0x8B, 0xC1, 0x40]);
    assert_eq!(a.jump_tables[&0x11006].kind, SwitchKind::Unbounded);
    // mov eax, 3 alone is no mask.
    let a = switch_after(&[0xB8, 0x03, 0, 0, 0]);
    assert_eq!(a.jump_tables[&0x11005].kind, SwitchKind::Unbounded);
}

#[test]
fn a_masked_table_skips_a_slot_that_cannot_be_a_case() {
    // The CRT memcpy shape: on this path `edi & 3` isn't 0, so the table
    // starts inside the `jmp`, at its own displacement. Slot 0 is code and
    // points into the middle of an instruction; it is a hole, not the end
    // of the table, and stays the `jmp`'s bytes.
    let words: Vec<u8> = [0x11030u32, 0x11031, 0x11032].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xF7, 0xC7, 0x03, 0x00, 0x00, 0x00]),       // test edi, 3
        (0x11006, &[0x74, 0x28]),                               // je 0x11030
        (0x11008, &[0x8B, 0xC7]),                               // mov eax, edi
        (0x1100A, &[0x83, 0xE0, 0x03]),                         // and eax, 3
        (0x1100D, &[0xFF, 0x24, 0x85, 0x10, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11010]
        (0x11014, &words),
        (0x11030, &[0xC3, 0xC3, 0xC3]),
    ]);
    let (a, cfgs) = cfgs_of(code);
    let jt = &a.jump_tables[&0x1100D];
    assert_eq!((jt.kind, jt.count, jt.bound), (SwitchKind::Bounded, 4, Some(4)));
    assert_eq!(jt.holes, [0]);
    assert_eq!(jt.cases().collect::<Vec<_>>(), [(1, 0x11030), (2, 0x11031), (3, 0x11032)]);
    assert_eq!(a.claim(0x11010), Some(('i', Some(TEXT))), "slot 0 is still the jmp's displacement");
    assert_eq!(a.claim(0x11014), Some(('d', Some(TEXT))));
    assert!(!a.functions.contains_key(&0x11031), "cases are not functions");
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    assert!(cfgs[&TEXT].errors.is_empty(), "{:?}", cfgs[&TEXT].errors);
}

#[test]
fn a_mask_wider_than_its_table_does_not_swallow_a_function() {
    // `and eax, 7` but only three cases: the next slot is a function the
    // caller calls first. The mask doesn't size the table, it only caps
    // the read, and the function is left alone.
    let words: Vec<u8> = [0x11060u32, 0x11061, 0x11062].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0xE8, 0x27, 0x00, 0x00, 0x00]),             // call 0x1102C
        (0x11005, &[0x83, 0xE0, 0x07]),                         // and eax, 7
        (0x11008, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x11020, &words),
        (0x1102C, &[0x33, 0xC0, 0xC3]), // xor eax, eax; ret
        (0x11060, &[0xC3, 0xC3, 0xC3]),
    ]);
    let (a, _) = cfgs_of(code);
    let jt = &a.jump_tables[&0x11008];
    assert_eq!((jt.kind, jt.count, &jt.targets), (SwitchKind::Unbounded, 3, &vec![0x11060, 0x11061, 0x11062]));
    assert!(a.functions.contains_key(&0x1102C));
    assert_eq!(a.claim(0x1102C), Some(('I', Some(0x1102C))));
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

#[test]
fn jb_bounds_a_switch_on_its_taken_side() {
    // cmp eax, 4; jb 0x11006; ret
    let a = switch_after(&[0x83, 0xF8, 0x04, 0x72, 0x01, 0xC3]);
    assert_four_cases(&a, 0x11006);
}

/// `prefix` at `TEXT` (which branches to 0x11006 on the in-range side),
/// `body` at 0x11006 ending in `jmp [ecx*4+disp]`, four cases at
/// 0x11040..0x11043 in the table below `disp` at 0x11020, and words that
/// look like cases just outside it.
fn negative_index_switch(prefix: &[u8], body: &[u8]) -> Analysis {
    let words: Vec<u8> = [0x11050u32, 0x11040, 0x11041, 0x11042, 0x11043, 0x11050]
        .iter()
        .flat_map(|t| t.to_le_bytes())
        .collect();
    let code = pieces(&[
        (TEXT, prefix),
        (0x11006, body),
        (0x1101C, &words),
        (0x11040, &[0xC3, 0xC3, 0xC3, 0xC3]),
        (0x11050, &[0xFF, 0xFF]),
    ]);
    cfgs_of(code).0
}

fn assert_read_below(a: &Analysis, site: u32) {
    let jt = &a.jump_tables[&site];
    assert_eq!((jt.kind, jt.table, jt.count, jt.bound), (SwitchKind::Bounded, 0x11020, 4, Some(4)));
    assert_eq!(jt.targets, [0x11040, 0x11041, 0x11042, 0x11043]);
    assert_eq!(a.claim(0x1101C).unwrap().0, '?');
    assert_eq!(a.claim(0x11030).unwrap().0, '?');
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

#[test]
fn a_negated_index_reads_the_table_below_its_displacement() {
    // cmp ecx, 4; jb 0x11006; ret / neg ecx; jmp [ecx*4+0x1102C]: ecx is
    // -3..=0, the CRT memmove's backward dword tails.
    let a = negative_index_switch(
        &[0x83, 0xF9, 0x04, 0x72, 0x01, 0xC3],
        &[0xF7, 0xD9, 0xFF, 0x24, 0x8D, 0x2C, 0x10, 0x01, 0x00],
    );
    assert_read_below(&a, 0x11008);
}

#[test]
fn a_borrow_on_sub_bounds_a_negative_index() {
    // sub ecx, 4; jb 0x11006; ret / jmp [ecx*4+0x11030]: ecx is -4..=-1,
    // the CRT memcpy's copies under 4 bytes.
    let a = negative_index_switch(
        &[0x83, 0xE9, 0x04, 0x72, 0x01, 0xC3],
        &[0xFF, 0x24, 0x8D, 0x30, 0x10, 0x01, 0x00],
    );
    assert_read_below(&a, 0x11006);
    // With jbe the borrow says nothing about ecx = 0.
    let a = negative_index_switch(
        &[0x83, 0xE9, 0x04, 0x76, 0x01, 0xC3],
        &[0xFF, 0x24, 0x8D, 0x30, 0x10, 0x01, 0x00],
    );
    assert_ne!(a.jump_tables.get(&0x11006).map(|j| j.kind), Some(SwitchKind::Bounded));
}

#[test]
fn a_switch_reached_again_unguarded_keeps_its_cases() {
    // Case 0 calls a function that jumps back to the table jump, with no
    // guard on the way. Its read stops at the int3 fill and adds no case,
    // so the table keeps the guard's shape.
    let words: Vec<u8> = [0x11040u32, 0x11046, 0x11047, 0x11048].iter().flat_map(|t| t.to_le_bytes()).collect();
    let code = pieces(&[
        (0x11000, &[0x83, 0xF8, 0x03]),                         // cmp eax, 3
        (0x11003, &[0x77, 0x07]),                               // ja 0x1100C
        (0x11005, &[0xFF, 0x24, 0x85, 0x20, 0x10, 0x01, 0x00]), // jmp [eax*4+0x11020]
        (0x1100C, &[0xC3]),
        (0x11020, &words),
        (0x11040, &[0xE8, 0x0B, 0x00, 0x00, 0x00, 0xC3]), // call 0x11050; ret
        (0x11046, &[0xC3, 0xC3, 0xC3]),
        (0x11050, &[0xEB, 0xB3]), // jmp 0x11005
    ]);
    let (a, _) = cfgs_of(code);
    let jt = &a.jump_tables[&0x11005];
    assert_eq!((jt.kind, jt.count, &jt.targets), (SwitchKind::Bounded, 4, &vec![0x11040, 0x11046, 0x11047, 0x11048]));
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

/// The guard after `bytes`, decoded from `TEXT`.
fn guard_after(bytes: &[u8]) -> Guard {
    let mut guard = Guard::default();
    let mut info = InstructionInfoFactory::new();
    for i in Decoder::with_ip(32, bytes, TEXT as u64, DecoderOptions::NONE) {
        guard.observe(&i, &mut info);
    }
    guard
}

#[test]
fn partial_and_full_writes_forget_masks_and_constants() {
    // mov cl, 0xFF; mov ch, 0xFF; mov cx, 0xFFFF; and cx, 7; inc ecx; pop ecx
    let writes =
        [&[0xB1, 0xFF][..], &[0xB5, 0xFF], &[0x66, 0xB9, 0xFF, 0xFF], &[0x66, 0x83, 0xE1, 0x07], &[0x41], &[0x59]];
    for write in writes {
        // and ecx, 3; mov ecx, 3
        for prefix in [&[0x83, 0xE1, 3][..], &[0xB9, 3, 0, 0, 0]] {
            let code = [prefix, write].concat();
            let g = guard_after(&code);
            assert_eq!(g.mask(Register::ECX), None, "{code:x?}");
            assert_eq!(g.consts[gpr(Register::ECX).unwrap()], None, "{code:x?}");
        }
    }
}

#[test]
fn a_borrow_is_forgotten_when_its_register_is_rewritten() {
    // sub ecx, 4; mov ecx, 0: the borrow says nothing about this ecx.
    let jb = Decoder::with_ip(32, &[0x72, 0], TEXT as u64, DecoderOptions::NONE).decode();
    for write in [&[0xB9, 0, 0, 0, 0][..], &[0xB1, 0]] {
        let g = guard_after(&[&[0x83, 0xE9, 4][..], write].concat());
        assert_eq!(g.taken(&jb).window, None, "{write:x?}");
    }
    // The table it would have shifted is read from its displacement.
    let words: Vec<u8> = (0x11040u32..0x11045).flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x83, 0xE9, 4, 0xB9, 0, 0, 0, 0, 0x72, 1, 0xC3]), // sub ecx, 4; mov ecx, 0; jb +1; ret
        (0x1100B, &[0xFF, 0x24, 0x8D, 0x30, 0x10, 1, 0]),          // jmp [ecx*4+0x11030]
        (0x11020, &words),
        (0x11040, &[0xC3; 5]),
    ]))
    .0;
    let jt = &a.jump_tables[&0x1100B];
    assert!(jt.targets.contains(&0x11044), "{jt:x?}");
}

/// `prefix` at `TEXT`, then `jmp [eax*4+0x11040]` over eight cases at
/// 0x11080..0x11087.
fn eight_cases(prefix: &[u8]) -> Analysis {
    let words: Vec<u8> = (0x11080u32..0x11088).flat_map(u32::to_le_bytes).collect();
    let jmp = [0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0];
    cfgs_of(pieces(&[(TEXT, prefix), (TEXT + prefix.len() as u32, &jmp), (0x11040, &words), (0x11080, &[0xC3; 8])])).0
}

#[test]
fn a_call_forgets_the_mask_on_what_the_callee_may_return_in() {
    // and eax, 3; call [0x12010]: the callee may return eax = 4.
    let a = eight_cases(&[0x83, 0xE0, 3, 0xFF, 0x15, 0x10, 0x20, 1, 0]);
    let jt = &a.jump_tables[&0x11009];
    assert!(jt.targets.contains(&0x11084), "{jt:x?}");
    // The same with a direct call to a function already explored, which
    // does return eax = 4.
    let words: Vec<u8> = (0x11080u32..0x11088).flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        (TEXT, &[0xE8, 0x8B, 0, 0, 0]),                    // call 0x11090
        (0x11005, &[0x83, 0xE0, 3, 0xE8, 0x83, 0, 0, 0]),  // and eax, 3; call 0x11090
        (0x1100D, &[0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]), // jmp [eax*4+0x11040]
        (0x11040, &words),
        (0x11080, &[0xC3; 8]),
        (0x11090, &[0xB8, 4, 0, 0, 0, 0xC3]), // mov eax, 4; ret
    ]))
    .0;
    let jt = &a.jump_tables[&0x1100D];
    assert!(jt.targets.contains(&0x11084), "{jt:x?}");
}

#[test]
fn a_fallthrough_into_the_block_drops_the_taken_guard() {
    // cmp eax, 4; jb dispatch; mov eax, 4; dispatch: eax = 4 is a case.
    let a = eight_cases(&[0x83, 0xF8, 4, 0x72, 5, 0xB8, 4, 0, 0, 0]);
    let jt = &a.jump_tables[&0x1100A];
    assert!(jt.targets.contains(&0x11084), "{jt:x?}");
}

#[test]
fn a_later_way_into_the_block_drops_the_taken_guard() {
    // The taken `jb` reaches dispatch first; a path found later sets
    // eax = 4 and falls into it.
    let words: Vec<u8> = (0x11080u32..0x11088).flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        // test edx, edx; jz 0x11010; cmp eax, 4; jb 0x11020; ret
        (TEXT, &[0x85, 0xD2, 0x74, 0x0C, 0x83, 0xF8, 4, 0x72, 0x17, 0xC3]),
        (0x11010, &[0xB8, 4, 0, 0, 0, 0xEB, 0x09]), // mov eax, 4; jmp 0x1101F
        (0x1101F, &[0x90, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]), // nop; jmp [eax*4+0x11040]
        (0x11040, &words),
        (0x11080, &[0xC3; 8]),
    ]))
    .0;
    let jt = &a.jump_tables[&0x11020];
    assert!(jt.targets.contains(&0x11084), "{jt:x?}");
}

#[test]
fn another_function_with_a_wider_guard_widens_a_shared_table() {
    // The entry allows eax <= 3 at the shared jump; a function its case 0
    // calls allows eax <= 7 and jumps there too.
    let words: Vec<u8> = [0x11080u32, 0x11090, 0x11091, 0x11092, 0x11093, 0x11094, 0x11095, 0x11096]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let a = cfgs_of(pieces(&[
        // cmp eax, 3; ja 0x1100C; jmp [eax*4+0x11040]; ret
        (TEXT, &[0x83, 0xF8, 3, 0x77, 0x07, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0, 0xC3]),
        (0x11040, &words),
        (0x11080, &[0xE8, 0x1B, 0, 0, 0, 0xC3]), // call 0x110A0; ret
        (0x11090, &[0xC3; 7]),
        // cmp eax, 7; ja 0x110AA; jmp 0x11005; ret
        (0x110A0, &[0x83, 0xF8, 7, 0x77, 0x07, 0xE9, 0x5B, 0xFF, 0xFF, 0xFF, 0xC3, 0xC3, 0xC3]),
    ]))
    .0;
    let jt = &a.jump_tables[&0x11005];
    assert!(jt.targets.contains(&0x11096), "{jt:x?}");
}

#[test]
fn a_hole_between_cases_does_not_lead_into_the_next_table() {
    // `and eax, 7; test eax, 6; jnz`: only cases 0 and 1 are reached, both
    // endless loops. A metadata word follows, then a table of pointers to
    // a known returning function.
    let words: Vec<u8> = [0x11070u32, 0x11072, 0xDEAD_BEEF, 0x11090, 0x11090, 0x11090, 0x11090, 0x11090]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let a = cfgs_of(pieces(&[
        // call 0x11090; and eax, 7; test eax, 6; jnz 0x11030; jmp [eax*4+0x11040]
        (TEXT, &[0xE8, 0x8B, 0, 0, 0, 0x83, 0xE0, 7, 0xA9, 6, 0, 0, 0, 0x75, 0x21, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]),
        (0x11030, &[0xEB, 0xFE]),
        (0x11040, &words),
        (0x11070, &[0xEB, 0xFE, 0xEB, 0xFE]),
        (0x11090, &[0xC3]),
    ]))
    .0;
    let jt = &a.jump_tables[&0x1100F];
    assert_eq!(jt.targets, [0x11070, 0x11072]);
}

#[test]
fn a_mask_skips_at_most_four_leading_holes() {
    for n in [4usize, 5] {
        let mut words = vec![0xDEAD_BEEFu32; n];
        words.extend(std::iter::repeat_n(0x11080, 8 - n));
        let words: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        // and eax, 7; jmp [eax*4+0x11040]
        let code = [0x83, 0xE0, 7, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0];
        let a = cfgs_of(pieces(&[(TEXT, &code), (0x11040, &words), (0x11080, &[0xC3])])).0;
        if n == 4 {
            assert_eq!(a.jump_tables[&0x11003].holes, [0, 1, 2, 3]);
        } else {
            assert!(!a.jump_tables.contains_key(&0x11003));
        }
    }
}

#[test]
fn a_negated_jbe_bound_includes_both_ends() {
    // cmp ecx, 3; jbe; neg ecx: -3..=0.
    let a = negative_index_switch(&[0x83, 0xF9, 3, 0x76, 1, 0xC3], &[0xF7, 0xD9, 0xFF, 0x24, 0x8D, 0x2C, 0x10, 1, 0]);
    assert_read_below(&a, 0x11008);
}

#[test]
fn jb_after_cmp_with_zero_or_all_ones_is_no_bound() {
    for prefix in [&[0x83, 0xF8, 0, 0x72, 1, 0xC3][..], &[0x83, 0xF8, 0xFF, 0x72, 1, 0xC3]] {
        let a = switch_after(prefix);
        assert_ne!(a.jump_tables.get(&0x11006).map(|j| j.kind), Some(SwitchKind::Bounded));
    }
}

/// Eight cases: 0x11080, then 0x11090..0x11096.
fn eight_words() -> Vec<u8> {
    [0x11080u32, 0x11090, 0x11091, 0x11092, 0x11093, 0x11094, 0x11095, 0x11096]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn assert_has_case(a: &Analysis, site: u32, case: u32) {
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
    let jt = &a.jump_tables[&site];
    assert!(jt.targets.contains(&case), "{case:#x} missing: {jt:x?}");
}

#[test]
fn a_mask_is_dropped_when_a_later_path_bypasses_it() {
    // test edx, edx; jz late; and eax, 3; dispatch: jmp [eax*4+0x11040]
    // late: mov eax, 4; jmp dispatch
    let words: Vec<u8> = (0x11080u32..0x11088).flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x85, 0xD2, 0x74, 0x0C, 0x83, 0xE0, 3, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]),
        (0x11010, &[0xB8, 4, 0, 0, 0, 0xEB, 0xF0]),
        (0x11040, &words),
        (0x11080, &[0xC3; 8]),
    ]))
    .0;
    assert_has_case(&a, 0x11007, 0x11084);
}

#[test]
fn an_entry_guard_is_dropped_when_a_later_path_joins_the_run() {
    // The taken `jb` guards the run from 0x11020; a later path sets
    // eax = 4 and joins it at the jump, past its start.
    let words: Vec<u8> = (0x11080u32..0x11088).flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x85, 0xD2, 0x74, 0x0C, 0x83, 0xF8, 4, 0x72, 0x17, 0xC3]),
        (0x11010, &[0xB8, 4, 0, 0, 0, 0xEB, 0x0B]),
        (0x11020, &[0x90, 0x90, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]),
        (0x11040, &words),
        (0x11080, &[0xC3; 8]),
    ]))
    .0;
    assert_has_case(&a, 0x11022, 0x11084);
}

#[test]
fn a_later_bounded_read_does_not_shrink_a_table() {
    // The entry jumps to the shared dispatch with eax = 7; case 0 calls a
    // function that reaches it with eax <= 3.
    let a = cfgs_of(pieces(&[
        (TEXT, &[0xB8, 7, 0, 0, 0, 0xEB, 0x1E]), // mov eax, 7; jmp 0x11025
        (0x11020, &[0x83, 0xF8, 3, 0x77, 7, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0, 0xC3]),
        (0x11040, &eight_words()),
        (0x11080, &[0xE8, 0x9B, 0xFF, 0xFF, 0xFF, 0xC3]), // call 0x11020; ret
        (0x11090, &[0xC3; 7]),
    ]))
    .0;
    assert_has_case(&a, 0x11025, 0x11096);
}

#[test]
fn a_table_keeps_both_negative_and_nonnegative_reads() {
    // cmp ecx, 4; jb; neg ecx reads below the displacement; a function
    // case 0 calls reaches the jump with ecx = 2.
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x83, 0xF9, 4, 0x72, 3, 0xC3]),
        (0x11008, &[0xF7, 0xD9, 0xFF, 0x24, 0x8D, 0x4C, 0x10, 1, 0]), // neg ecx; jmp [ecx*4+0x1104C]
        (0x11040, &eight_words()),
        (0x11080, &[0xE8, 0x3B, 0, 0, 0, 0xC3]), // call 0x110C0; ret
        (0x11090, &[0xC3; 7]),
        (0x110C0, &[0xB9, 2, 0, 0, 0, 0xE9, 0x40, 0xFF, 0xFF, 0xFF]), // mov ecx, 2; jmp 0x1100A
    ]))
    .0;
    assert_has_case(&a, 0x1100A, 0x11080);
    assert_has_case(&a, 0x1100A, 0x11095);
}

#[test]
fn a_speculative_visitor_adds_its_cases() {
    // The entry masks eax to 3; a function found from an immediate in
    // case 0 reaches the jump with eax = 7.
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x83, 0xE0, 3, 0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]),
        (0x11040, &eight_words()),
        (0x11080, &[0x68, 0xA0, 0x10, 1, 0, 0x5A, 0xC3]), // push 0x110A0; pop edx; ret
        (0x11090, &[0xC3; 7]),
        // mov eax, 7; and eax, 7; jmp 0x11003
        (0x110A0, &[0xB8, 7, 0, 0, 0, 0x83, 0xE0, 7, 0xE9, 0x56, 0xFF, 0xFF, 0xFF]),
    ]))
    .0;
    assert_eq!(a.functions.get(&0x110A0).map(|f| f.source), Some(Source::CodeRef));
    assert_has_case(&a, 0x11003, 0x11096);
}

#[test]
fn cyclic_taken_edges_terminate() {
    let words: Vec<u8> = [0x11010u32, 0x11080, 0x11081, 0x11082].into_iter().flat_map(u32::to_le_bytes).collect();
    let a = cfgs_of(pieces(&[
        (TEXT, &[0x83, 0xF8, 4, 0x72, 0x0B, 0xC3]),
        (0x11010, &[0x83, 0xF8, 4, 0x72, 0x0B, 0xC3]),
        (0x11020, &[0xFF, 0x24, 0x85, 0x40, 0x10, 1, 0]),
        (0x11040, &words),
        (0x11080, &[0xC3; 3]),
    ]))
    .0;
    assert!(a.converged);
    assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
}

#[test]
fn a_shared_table_is_the_same_whatever_order_it_is_visited_in() {
    // A root and a function its case 0 calls reach one jump, each bounded
    // (jb at 8 or 4) or with eax = 7, in both orders: always eight cases.
    let mut counts = Vec::new();
    for (first, last) in [(Some(8), 4u8), (Some(4), 8), (None, 4), (Some(4), 0)] {
        let root = first.map_or(vec![0xB8, 7, 0, 0, 0, 0xEB, 0x29], |n| vec![0x83, 0xF8, n, 0x72, 0x2B, 0xC3]);
        let visitor = match last {
            0 => vec![0xB8, 7, 0, 0, 0, 0xEB, 9],
            n => vec![0x83, 0xF8, n, 0x72, 0x0B, 0xC3],
        };
        let words = [0x11080u32, 0x11090, 0x11091, 0x11092, 0x11093, 0x11094, 0x11095, 0x11096];
        let words: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        let a = cfgs_of(pieces(&[
            (TEXT, &root),
            (0x11020, &visitor),
            (0x11030, &[0xFF, 0x24, 0x85, 0x60, 0x10, 1, 0]), // jmp [eax*4+0x11060]
            (0x11060, &words),
            (0x11080, &[0xE8, 0x9B, 0xFF, 0xFF, 0xFF, 0xC3]), // call 0x11020; ret
            (0x11090, &[0xC3; 7]),
        ]))
        .0;
        assert!(a.conflicts.is_empty(), "{:?}", a.conflicts);
        counts.push(a.jump_tables[&0x11030].count);
    }
    assert_eq!(counts, [8, 8, 8, 8]);
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
