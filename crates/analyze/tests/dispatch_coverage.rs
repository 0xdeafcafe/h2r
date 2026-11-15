//! Runtime-decoded retail coverage; no game bytes/tables are test fixtures.
use analyze::dispatch::{CAMPAIGN_ROOTS, catalog, coverage, entries};
#[test]
#[ignore = "needs retail default.xbe in $H2_XBE"]
fn campaign_tables_have_dispatch_entries() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let before = analyze::analyze_with_pointer_seeds(&xbe, false).unwrap();
    let after = analyze::analyze(&xbe).unwrap();
    assert!(after.converged);
    assert!(after.conflicts.is_empty());
    // Intentionally trapping Havok callbacks need new function bodies.
    for target in [
        0x1c2360, 0x1c2370, 0x1c2380, 0x1ea960, 0x1ea970, 0x1ea980, 0x1ea990, 0x1ea9a0, 0x1ea9b0,
    ] {
        assert!(
            after.functions.contains_key(&target),
            "missing function {target:#x}"
        );
    }
    // A numeric/string word next to a vtable must not become a function seed.
    assert!(!after.functions.contains_key(&0x2d2d2d));
    assert!(!after.functions.contains_key(&0xa000d));
    let tables = catalog(&xbe, &after).unwrap();
    assert!(tables.iter().any(|t| t.name == "haloscript-evaluate"));
    let exported = entries(&xbe, &after);
    // Existing instruction boundaries become externally callable entries.
    for target in [0x2305f5, 0x354c28, 0x354e28, 0x354f28] {
        assert!(exported.contains(&target), "missing entry {target:#x}");
    }
    assert!(!exported.contains(&0xa000d));
    let original_entries = entries(&xbe, &before);
    assert!(
        original_entries.is_subset(&exported),
        "lost entries: {:x?}",
        original_entries.difference(&exported).collect::<Vec<_>>()
    );
    for table in tables.iter().filter(|t| t.known_layout) {
        for &(_, target) in &table.slots {
            assert!(
                target == 0 || exported.contains(&target),
                "{} misses {target:#x}",
                table.name
            );
        }
    }
    for root in CAMPAIGN_ROOTS {
        assert!(after.functions.contains_key(&root));
        let c = coverage(&xbe, &after, &tables, &[root]);
        assert!(
            c.cfg_errors.is_empty(),
            "root {root:#x}: {:?}",
            c.cfg_errors
        );
        assert!(!c.sites.is_empty());
    }
    println!(
        "functions {} -> {}; entries {} -> {}",
        before.functions.len(),
        after.functions.len(),
        original_entries.len(),
        exported.len()
    );
}
