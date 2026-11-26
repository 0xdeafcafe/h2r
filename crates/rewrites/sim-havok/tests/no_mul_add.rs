//! Retail's arithmetic is separate multiplies and adds, never fused
//! (docs/engine/sim-rewrite.md section 3). The crate's `clippy.toml` denies
//! the calls; this greps the sources as a backstop for a build that skips
//! clippy.

use std::fs;
use std::path::Path;

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_mul_add_in_kernel() {
    // Built in two parts so this file does not match itself.
    let needle = concat!("mul", "_add");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    assert!(!files.is_empty());
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains(needle), "{} uses a fused multiply-add", file.display());
    }
    let clippy = fs::read_to_string(root.join("clippy.toml")).unwrap();
    for method in ["f32::", "f64::"] {
        assert!(clippy.contains(&format!("{method}{needle}")), "clippy.toml must deny {method}{needle}");
    }
}
