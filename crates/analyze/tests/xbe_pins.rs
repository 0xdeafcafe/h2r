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
