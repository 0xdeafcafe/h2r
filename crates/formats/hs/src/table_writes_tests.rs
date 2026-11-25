//! The writes over invented tables: an edit writes only its own nodes, a
//! growth is a copy, and either way the saved bytes read back as the new
//! tables. No game data.

use super::*;
use crate::compiler::tests::{setup, tables};
use crate::{Options, Script, compile};

const BASE: u32 = 0x4000_0000;

/// The invented tables in a payload, with room for the tables to be
/// patched, and the header the writes read.
fn payload() -> Vec<u8> {
    let mut p = vec![0u8; 0x8000];
    tables().write(&mut p, BASE, HS_HEADER_SIZE as usize);
    p
}

/// What a save does: the patches in place, each grown table appended with
/// its pair pointed at the copy.
fn save(payload: &mut Vec<u8>, w: &Writes) {
    for (a, b) in &w.patches {
        let at = (a - BASE) as usize;
        payload[at..at + b.len()].copy_from_slice(b);
    }
    for g in &w.grown {
        let at = payload.len().next_multiple_of(16);
        payload.resize(at, 0);
        payload.extend_from_slice(&g.bytes);
        let root = (g.root - BASE) as usize;
        payload[root..root + 4].copy_from_slice(&g.count.to_le_bytes());
        payload[root + 4..root + 8].copy_from_slice(&(BASE + at as u32).to_le_bytes());
    }
}

fn header(p: &[u8]) -> Vec<u8> {
    p[..HS_HEADER_SIZE as usize].to_vec()
}

#[test]
fn unchanged_tables_write_nothing() {
    let p = payload();
    let (s, _) = setup();
    assert!(writes(&header(&p), BASE, &s, &s).unwrap().is_empty());
}

#[test]
fn an_edit_writes_only_its_own_nodes() {
    let mut p = payload();
    let (s, catalog) = setup();
    let out = compile(
        &s,
        &catalog,
        "(script static boolean other (invented_flag))",
        &Options::default(),
    )
    .unwrap();
    let w = writes(&header(&p), BASE, &s, &out.scenario).unwrap();
    assert!(w.grown.is_empty(), "{w:?}");
    // Every patch lies in the node table, on a slot `other` used.
    let nodes = word(&p, 0x238 + 4);
    let root = s.scripts()[1].root();
    let head = crate::Datum(s.node(root).unwrap().data());
    let used = [root.slot(), head.slot()];
    assert!(!w.patches.is_empty());
    // The one script table patch is `other`'s root, which the recompile
    // gave a new salt.
    let script = word(&p, 0x1b8 + 4) + Script::SIZE as u32;
    let mut script_patches = 0;
    for (a, b) in &w.patches {
        if *a >= script && *a + b.len() as u32 <= script + Script::SIZE as u32 {
            script_patches += 1;
            continue;
        }
        assert!(*a >= nodes, "{a:#x} is outside the node table");
        let (first, last) = ((a - nodes) as usize / 20, (a - nodes) as usize + b.len() - 1);
        for slot in first..=last / 20 {
            assert!(used.contains(&slot), "slot {slot} was written");
        }
    }
    assert_eq!(script_patches, 1, "{w:?}");
    save(&mut p, &w);
    let back = Scenario::decode(&p, BASE).unwrap();
    assert_eq!(back.table_bytes(), out.scenario.table_bytes());
    back.verify_codec_symmetry(&p, BASE).unwrap();
}

#[test]
fn a_new_script_and_string_are_written_whole_to_new_places() {
    let mut p = payload();
    let (s, catalog) = setup();
    let out = compile(
        &s,
        &catalog,
        "(script dormant later (invented_print \"brand new\" 1))",
        &Options::default(),
    )
    .unwrap();
    let w = writes(&header(&p), BASE, &s, &out.scenario).unwrap();
    let mut roots: Vec<u32> = w.grown.iter().map(|g| g.root - BASE).collect();
    roots.sort();
    // Strings, scripts and nodes outgrew the map's; globals and tag
    // references did not change.
    assert_eq!(roots, [0x1b0, 0x1b8, 0x238]);
    let scripts = w.grown.iter().find(|g| g.root == BASE + 0x1b8).unwrap();
    assert_eq!((scripts.count, scripts.bytes.len()), (3, 3 * 40));
    save(&mut p, &w);
    let back = Scenario::decode(&p, BASE).unwrap();
    assert_eq!(back.table_bytes(), out.scenario.table_bytes());
    back.verify_codec_symmetry(&p, BASE).unwrap();
}

#[test]
fn a_header_that_does_not_describe_the_tables_is_refused() {
    let (s, catalog) = setup();
    let out = compile(
        &s,
        &catalog,
        "(script static boolean other (invented_flag))",
        &Options::default(),
    )
    .unwrap();
    let e = writes(&[0; 0x240], BASE, &s, &out.scenario).unwrap_err();
    assert!(e.to_string().contains("header describes"), "{e}");
    assert!(writes(&[0; 8], BASE, &s, &out.scenario).is_err());
}
