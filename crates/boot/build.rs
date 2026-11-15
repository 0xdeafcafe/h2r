//! Source hashes (FNV-1a over file names and contents), so cached results
//! are dropped whenever what produced them can have changed:
//!
//! - `UNREAD_SOURCE_HASH`: the analyze crate and the boot's fallback, for
//!   the cached unread-target set (fallback.rs).
//! - `GENERATOR_SOURCE_HASH`: everything that turns an XBE into the native
//!   module (xbe, analyze, ir, lift, emit and its C, x87, the runtime ABI
//!   header and the boot's bridge C), for the dylib's build stamp (lib.rs).

use std::path::{Path, PathBuf};

fn walk(dir: &Path, exts: &[&str], files: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, exts, files);
        } else if p.extension().is_some_and(|x| exts.iter().any(|e| x == *e)) {
            files.push(p);
        }
    }
}

/// Hash every file with one of `exts` under `dirs`, plus `extra` files.
fn hash(dirs: &[&str], exts: &[&str], extra: &[&str]) -> u64 {
    let mut files = Vec::new();
    for d in dirs {
        println!("cargo:rerun-if-changed={d}");
        walk(Path::new(d), exts, &mut files);
    }
    files.extend(extra.iter().map(PathBuf::from));
    files.sort();
    files.dedup();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        for &b in f.to_string_lossy().as_bytes().iter().chain(&std::fs::read(f).unwrap_or_default()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn main() {
    let unread = hash(&["../analyze/src"], &["rs"], &["src/fallback.rs"]);
    println!("cargo:rustc-env=UNREAD_SOURCE_HASH={unread:016x}");
    let generator = hash(
        &["../xbe/src", "../analyze/src", "../ir/src", "../lift/src", "../emit/src", "../emit/c", "../x87/src", "../runtime/include", "c"],
        &["rs", "c", "h"],
        &[],
    );
    println!("cargo:rustc-env=GENERATOR_SOURCE_HASH={generator:016x}");
}
