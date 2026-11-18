//! The field registry against the owner's disc. Opt-in: `H2_DISC` names
//! the ISO. Nothing is written anywhere.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test --release -p tags --test tag_registry -- --ignored --nocapture
//! ```
//!
//! Every group of every tag with metadata on every map has a layout, every
//! tag is at least as long as its root, and every nonempty block or data
//! reference of every element, walked through the registry, points at
//! whole elements inside the metadata.

use map::{FourCC, Map};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use tags::registry::{Kind, Layout, Registry};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Walks `count` elements of `l` at `addr`; returns the elements seen.
fn walk(meta: &[u8], base: u32, l: &Layout, addr: u32, count: u32, bad: &mut Vec<String>, path: &str) -> u64 {
    let mut seen = u64::from(count);
    for i in 0..count {
        let at = (addr - base) as usize + (i * l.size) as usize;
        for f in &l.fields {
            let o = at + f.offset as usize;
            match &f.kind {
                Kind::Block(child) => {
                    let (n, a) = (u32_at(meta, o), u32_at(meta, o + 4));
                    if n == 0 {
                        continue;
                    }
                    let end = u64::from(a) + u64::from(n) * u64::from(child.size);
                    if a < base || end > u64::from(base) + meta.len() as u64 {
                        bad.push(format!("{path}.{}[{i}]: {n} elements at {a:#x} outside", f.name));
                        continue;
                    }
                    seen += walk(meta, base, child, a, n, bad, &format!("{path}.{}", f.name));
                }
                Kind::Data => {
                    let (n, a) = (u32_at(meta, o), u32_at(meta, o + 4));
                    if n != 0 && (a < base || u64::from(a) + u64::from(n) > u64::from(base) + meta.len() as u64) {
                        bad.push(format!("{path}.{}[{i}]: {n} bytes at {a:#x} outside", f.name));
                    }
                }
                _ => {}
            }
        }
    }
    seen
}

#[test]
#[ignore = "needs the owner's disc: set H2_DISC"]
fn every_disc_tag_walks_through_the_registry() -> Res<()> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
    let mut maps = Vec::new();
    for e in iso.entries()? {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        if file.ends_with(".map") && !e.is_dir {
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            maps.push((file.to_owned(), base, u64::from(e.size)));
        }
    }
    let r = Registry::builtin();
    let mut bad = Vec::new();
    let mut by_group: BTreeMap<FourCC, (u64, u64)> = BTreeMap::new();
    for (name, base, size) in &maps {
        let mut m = Map::from_region(BufReader::new(File::open(&disc)?), *base, *size)?;
        let Some(space) = m.metadata_space() else { continue };
        let meta = m.read_file_range(space.file_offset, u64::from(space.size))?;
        for t in m.tags().to_vec() {
            let Some(loc) = t.location else { continue };
            if loc.space != space {
                continue; // BSP regions: their own spaces.
            }
            let path = format!("{name}:{}.{}", t.name.as_deref().unwrap_or("?"), tags::defs::fourcc_text(t.group));
            let Some(l) = r.group(t.group) else {
                bad.push(format!("{path}: no layout"));
                continue;
            };
            if loc.size < l.size {
                bad.push(format!("{path}: {:#x} bytes, root {:#x}", loc.size, l.size));
                continue;
            }
            let n = walk(&meta, space.virtual_base, l, loc.address, 1, &mut bad, &path);
            let e = by_group.entry(t.group).or_default();
            e.0 += 1;
            e.1 += n;
        }
    }
    for (g, (tags, elements)) in &by_group {
        let c = r.group(*g).unwrap().coverage();
        println!(
            "{:>4}: {tags:>6} tags {elements:>9} elements; fields named {:>4}, unnamed {:>5}",
            tags::defs::fourcc_text(*g),
            c.named_fields,
            c.unnamed_fields
        );
    }
    let c = r.coverage();
    println!(
        "{} groups on the disc; registry {} named fields, {} unnamed",
        by_group.len(),
        c.named_fields,
        c.unnamed_fields
    );
    bad.truncate(50);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    Ok(())
}
