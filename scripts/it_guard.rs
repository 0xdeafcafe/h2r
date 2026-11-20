// Included at the end of a crate's `tests/it.rs`, the one binary its
// integration test files build into (docs/dev/build-speed.md).

/// Such a crate sets `autotests = false`, so cargo builds a file under
/// `tests/` only when `it.rs` names it as a module or Cargo.toml gives it a
/// `[[test]]`. A file with neither would never run, and nothing would say so.
#[test]
fn every_test_file_is_built() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = std::fs::read_to_string(dir.join("tests/it.rs")).unwrap();
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(dir.join("tests")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if name == "it.rs" || !name.ends_with(".rs") {
            continue;
        }
        let module = root.contains(&format!("#[path = \"{name}\"]"));
        let own_binary = manifest.contains(&format!("path = \"tests/{name}\""));
        if !module && !own_binary {
            missing.push(name);
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "tests/{missing:?}: add `#[path = \"<file>\"] mod <name>;` to tests/it.rs, \
         or a `[[test]]` with `path = \"tests/<file>\"` to Cargo.toml"
    );
}
