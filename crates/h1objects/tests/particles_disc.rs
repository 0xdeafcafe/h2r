//! Halo 1 particles (`part`, `pctl`) and contrails (`cont`) as Halo 2
//! `prt3` and `cont`: surveys of the Halo 1 layouts and the proofs of the
//! conversion, from the owner's discs (`$H1_ISO`, `$H2_DISC`).
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Block, Group, H1Map, TagRef};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Every Halo 1 map on the disc (`H1_LEVELS` for some), by stem.
fn h1_maps() -> Vec<(String, H1Map)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let entries = iso.entries().unwrap();
    let mut out = Vec::new();
    for e in h1map::disc::maps(&entries) {
        let stem = h1map::disc::map_stem(&e).to_ascii_lowercase();
        if let Ok(only) = std::env::var("H1_LEVELS")
            && !only.split(',').any(|l| l == stem)
        {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        if let Ok(m) = H1Map::from_reader(Cursor::new(bytes)) {
            out.push((stem, m));
        }
    }
    out
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Survey (`H1_GROUP`, default `part`; `H1_SPAN` bytes, default 0x180):
/// per 4-byte offset of every distinct tag of the group, the references
/// by group, the block fields (count range), and the words: nonzero
/// count, float range, distinct values when few.
#[test]
#[ignore]
fn halo1_layout_survey() {
    let g = std::env::var("H1_GROUP").unwrap_or_else(|_| "part".into());
    let group = Group::new(g.as_bytes().try_into().unwrap());
    let span = std::env::var("H1_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x180);
    let mut seen = std::collections::BTreeSet::new();
    let mut tags = 0;
    let mut refs: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut words: BTreeMap<usize, BTreeMap<u32, usize>> = BTreeMap::new();
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == group) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            tags += 1;
            *lens.entry(b.len()).or_default() += 1;
            for at in (0..span.min(b.len().saturating_sub(4))).step_by(4) {
                *words.entry(at).or_default().entry(u32_at(b, at)).or_default() += 1;
                if at + 16 <= b.len()
                    && let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    let tail = x.name.rsplit('\\').next().unwrap_or("").to_owned();
                    *refs.entry(at).or_default().entry(format!("{} {tail}", r.group)).or_default() += 1;
                }
                if let Some(k) = Block::read(b, at)
                    && k.count > 0
                    && k.count < 0x400
                    && k.address >> 28 == 8
                {
                    *refs.entry(at).or_default().entry(format!("block? count {}", k.count)).or_default() += 1;
                }
            }
        }
    }
    println!("{g}: {tags} distinct tags, lengths {:?}", lens.iter().take(8).collect::<Vec<_>>());
    for (at, w) in &words {
        let nz: usize = w.iter().filter(|(v, _)| **v != 0).map(|(_, c)| c).sum();
        if nz == 0 {
            continue;
        }
        let floats: Vec<f32> = w
            .keys()
            .map(|v| f32::from_bits(*v))
            .filter(|f| f.is_finite() && f.abs() < 1e7 && (f.abs() > 1e-6 || *f == 0.0))
            .collect();
        let (lo, hi) = floats
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), f| (a.min(*f), b.max(*f)));
        let few: Vec<String> = if w.len() <= 6 {
            w.iter().map(|(v, c)| format!("{v:#x}x{c}")).collect()
        } else {
            vec![format!("{} distinct", w.len())]
        };
        let r = refs.get(at).map(|r| {
            let mut v: Vec<_> = r.iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
            format!("{:?}", &v[..v.len().min(3)])
        });
        println!(
            "{at:#05x}: nonzero {nz}, floats {}/{} {lo} ..= {hi}, {few:?} {}",
            floats.len(),
            w.len(),
            r.unwrap_or_default()
        );
    }
}
