//! Checks discovery against the real retail XBE. Ignored by default (the
//! image is not in the repository). Run with the path to a `default.xbe`,
//! read-only:
//!
//! `H2_XBE=/path/default.xbe cargo test -p analyze --test xbe_pins -- --ignored`
use xbe::Xbe;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn unguarded_enum_switches_keep_their_holes() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let data = std::fs::read(path).unwrap();
    let a = analyze::analyze(&Xbe::parse(&data).unwrap()).unwrap();
    // `dec eax; jmp [eax*4+table]` with no range check and null entries
    // for the values the enum never takes, cases between code and table.
    for (table, holes) in [(0x0021_6a88u32, [4u32, 5]), (0x0021_6b30, [2, 5])] {
        let jt = a.jump_tables.values().find(|t| t.table == table).unwrap();
        assert_eq!(jt.holes, holes, "table {table:#x}");
        assert!(jt.targets.iter().all(|t| *t < table), "table {table:#x}: {:x?}", jt.targets);
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn crt_copy_routines_resolve_every_jump_table() {
    use analyze::SwitchKind::{Bounded, Unbounded};
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let data = std::fs::read(path).unwrap();
    let x = Xbe::parse(&data).unwrap();
    let a = analyze::analyze(&x).unwrap();
    // The CRT memcpy/memmove at 0x320890 copies through six tables.
    // Leading and trailing bytes go through ones indexed after `and r, 3`;
    // those only reached with the low bits not 0 have a slot 0 that
    // overlaps code, a hole. The dword tails are indexed by a count below
    // 8, the backward one negated (`neg ecx`), so read below the
    // displacement. Forward copies under 4 bytes index the byte tails at
    // -4..-1 (`sub ecx, 4; jb`).
    const BYTES: [u32; 4] = [0x0032_09ec, 0x0032_09f4, 0x0032_0a00, 0x0032_0a14];
    const LEAD: [u32; 3] = [0x0032_0900, 0x0032_092c, 0x0032_0950];
    const DWORDS: [u32; 8] =
        [0x0032_09d3, 0x0032_09c0, 0x0032_09b8, 0x0032_09b0, 0x0032_09a8, 0x0032_09a0, 0x0032_0998, 0x0032_0990];
    const BACK_BYTES: [u32; 4] = [0x0032_0b88, 0x0032_0b90, 0x0032_0ba0, 0x0032_0bb4];
    const BACK_LEAD: [u32; 3] = [0x0032_0a8c, 0x0032_0ab0, 0x0032_0ad8];
    const BACK_DWORDS: [u32; 8] =
        [0x0032_0b2c, 0x0032_0b34, 0x0032_0b3c, 0x0032_0b44, 0x0032_0b4c, 0x0032_0b54, 0x0032_0b5c, 0x0032_0b6f];
    // (site, kind, first entry read, holes, targets). The `and edx, ecx`
    // sites mask with a constant set in another block, so they stay
    // unguarded; their reads still stop at the table's end.
    let sites: [(u32, _, u32, &[u32], &[u32]); 16] = [
        (0x0032_08c5, Bounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_08dd, Bounded, 0x0032_08f0, &[0], &LEAD),
        (0x0032_08e4, Bounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_08ec, Bounded, 0x0032_0970, &[], &DWORDS),
        (0x0032_0922, Unbounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_0948, Unbounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_0966, Unbounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_09d3, Unbounded, 0x0032_09dc, &[], &BYTES),
        (0x0032_0a4b, Bounded, 0x0032_0b78, &[], &BACK_BYTES),
        (0x0032_0a56, Bounded, 0x0032_0b0c, &[], &BACK_DWORDS),
        (0x0032_0a71, Bounded, 0x0032_0a7c, &[0], &BACK_LEAD),
        (0x0032_0a78, Bounded, 0x0032_0b78, &[], &BACK_BYTES),
        (0x0032_0aa6, Unbounded, 0x0032_0b78, &[], &BACK_BYTES),
        (0x0032_0ad0, Unbounded, 0x0032_0b78, &[], &BACK_BYTES),
        (0x0032_0b02, Unbounded, 0x0032_0b78, &[], &BACK_BYTES),
        (0x0032_0b6f, Unbounded, 0x0032_0b78, &[], &BACK_BYTES),
    ];
    for (site, kind, table, holes, targets) in sites {
        let jt = a.jump_tables.get(&site).unwrap_or_else(|| panic!("no table at {site:#x}"));
        assert_eq!(
            (jt.kind, jt.table, jt.holes.as_slice(), jt.targets.as_slice()),
            (kind, table, holes, targets),
            "site {site:#x}"
        );
    }
    let family = |va: u32| (0x0032_0890..0x0032_0bd0).contains(&va);
    let pinned = |va: u32| sites.iter().any(|s| s.0 == va);
    let extra: Vec<u32> = a.jump_tables.keys().copied().filter(|s| family(*s) && !pinned(*s)).collect();
    assert!(extra.is_empty(), "tables not pinned: {extra:x?}");
    // Every case is a block of the copy routine, not a function.
    let cfg = a.function_cfg(&x, 0x0032_0890).unwrap();
    assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
    for (_, _, _, _, targets) in sites {
        for t in targets {
            assert!(!a.functions.contains_key(t), "{t:#x} is a function");
            assert!(cfg.blocks.contains_key(t), "{t:#x} is not a block of the copy routine");
        }
    }
}
