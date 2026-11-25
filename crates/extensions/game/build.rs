//! The campaign cards' source hash (`CAMPAIGN_CARDS_SOURCE_HASH`, FNV-1a
//! over file names and contents), so a cached card is rebuilt whenever
//! the rewrite or the encoder it calls changes (campaign_cards/cache.rs).
//! The build's own hashes are `recomp`'s build script.

use std::path::{Path, PathBuf};

#[path = "../../../art/announcer/start_lines.rs"]
mod start_lines;

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
    ui_docs();
    start_clips();
}

/// Package the two type lines with the same mixer format as the medals.
fn start_clips() {
    let root = Path::new("../../../art/announcer");
    println!("cargo:rerun-if-changed={}", root.display());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    for id in ["firefight", "siege"] {
        let name = format!("{id}.wav");
        let path = root.join(&name);
        let authored = match std::fs::read(&path) {
            Ok(bytes) if !bytes.starts_with(b"version https://git-lfs") => Some(bytes),
            Ok(_) => None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => panic!("{}: {e}", path.display()),
        };
        let bytes = authored.unwrap_or_else(|| start_lines::wav(id).expect("known start line"));
        std::fs::write(out.join(name), bytes).expect("write start clip");
    }
}

/// Bake the screen docs for live presentation and the sample states for
/// previews. The map builders read their inputs from the package manifest.
fn ui_docs() {
    let root = Path::new("../../../content/ui");
    let screens = root.join("screens");
    println!("cargo:rerun-if-changed={}", root.display());
    println!("cargo:rerun-if-changed={}", screens.display());
    let abs = |p: &Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
            .display()
            .to_string()
    };
    let mut names: Vec<String> = std::fs::read_dir(&screens)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
        .collect();
    names.sort();
    let design = abs(&root.join("design.yaml"));
    println!("cargo:rerun-if-changed={design}");
    let mut out = format!("pub const DESIGN: &str = include_str!({design:?});\n");
    out.push_str("pub const SCREENS: &[(&str, &str)] = &[\n");
    for n in &names {
        let p = abs(&screens.join(format!("{n}.yaml")));
        println!("cargo:rerun-if-changed={p}");
        out.push_str(&format!("    ({n:?}, include_str!({p:?})),\n"));
    }
    out.push_str("];\n");
    let samples = Path::new("../../../assets/ui/retail-samples");
    println!("cargo:rerun-if-changed={}", samples.display());
    out.push_str("pub const RETAIL_SAMPLES: &[(&str, &str)] = &[\n");
    let mut paths = Vec::new();
    walk(samples, &["yaml"], &mut paths);
    paths.sort();
    for path in paths {
        let name = path.file_stem().and_then(|s| s.to_str()).expect("sample name");
        let p = abs(&path);
        println!("cargo:rerun-if-changed={p}");
        out.push_str(&format!("    ({name:?}, include_str!({p:?})),\n"));
    }
    out.push_str("];\n");
    let dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(dir.join("ui_baked.rs"), out).expect("ui_baked.rs");
}
