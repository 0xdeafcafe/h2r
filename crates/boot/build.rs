//! `UNREAD_SOURCE_HASH`: FNV-1a over the analyze crate's sources and the
//! boot's fallback, so a cached unread-target set (fallback.rs) is dropped
//! whenever the analysis or the reading of tables can have changed.

use std::path::Path;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

fn walk(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, files);
        } else if p.extension().is_some_and(|x| x == "rs") {
            files.push(p);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    walk(Path::new("../analyze/src"), &mut files);
    files.push("src/fallback.rs".into());
    files.sort();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        fnv(&mut h, f.to_string_lossy().as_bytes());
        fnv(&mut h, &std::fs::read(f).unwrap_or_default());
    }
    println!("cargo:rerun-if-changed=../analyze/src");
    println!("cargo:rustc-env=UNREAD_SOURCE_HASH={h:016x}");
}
