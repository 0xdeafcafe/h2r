//! The campaign cards' source hash (`CAMPAIGN_CARDS_SOURCE_HASH`, FNV-1a
//! over file names and contents), so a cached card is rebuilt whenever
//! the rewrite or the encoder it calls changes (campaign_cards/cache.rs).
//! The build's own hashes are `recomp`'s build script.

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
    // The card cache holds a built map and its tag IDs. Invalidate both
    // whenever the rewrite or the encoder it calls changes.
    let cards = hash(
        &["../../formats/map/src", "../../formats/tags/src", "src/campaign_cards"],
        &["rs"],
        &["src/campaign_cards.rs", "../../../Cargo.lock"],
    );
    println!("cargo:rustc-env=CAMPAIGN_CARDS_SOURCE_HASH={cards:016x}");
}
