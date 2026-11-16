//! Copies a subset of an XISO's files into a directory, keeping the tree:
//!
//!     cargo run -p xiso --example subset -- <image.iso> <out-dir> <pattern>...
//!     cargo run -p xiso --example subset -- <image.iso> --list
//!
//! A pattern is a path from the disc root (`maps/shared.map`) or a
//! directory ending in `/` (`fonts/`); see `xiso::subset_matches`. The tvOS
//! app bundles such a copy as its disc (docs/tvos.md). Files already the
//! right size are skipped, so a rerun is cheap.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [iso, out, patterns @ ..] = args.as_slice() else {
        return Err("usage: subset <image.iso> (<out-dir> <pattern>... | --list)".into());
    };
    let mut x = xiso::Xiso::open(BufReader::new(File::open(iso)?))?;
    let entries = x.entries()?;
    if out == "--list" {
        for e in &entries {
            println!(
                "{:>12} {}{}",
                e.size,
                e.path,
                if e.is_dir { "/" } else { "" }
            );
        }
        return Ok(());
    }
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let out = PathBuf::from(out);
    let (mut files, mut bytes) = (0u64, 0u64);
    for e in entries
        .iter()
        .filter(|e| xiso::subset_matches(&e.path, e.is_dir, &patterns))
    {
        let dest = out.join(e.path.trim_start_matches('/'));
        if e.is_dir {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        files += 1;
        bytes += e.size as u64;
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == e.size as u64) {
            continue;
        }
        let tmp = dest.with_extension("partial");
        x.copy_to(e, &mut File::create(&tmp)?)?;
        std::fs::rename(&tmp, &dest)?;
    }
    for p in &patterns {
        if !entries
            .iter()
            .any(|e| xiso::subset_matches(&e.path, e.is_dir, &[p]) && !e.is_dir)
        {
            eprintln!("subset: nothing on the disc matches {p}");
        }
    }
    eprintln!(
        "subset: {files} files, {:.1} MB in {}",
        bytes as f64 / 1e6,
        out.display()
    );
    Ok(())
}
