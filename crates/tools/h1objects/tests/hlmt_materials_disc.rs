//! The `hlmt` materials block (`+0x58`, `0x14` each) on retail maps, and
//! what a converted map's `hlmt`s must hold there. Prints names and counts
//! only.
//!
//! ```sh
//! H2_DISC=... cargo test -p h1objects --test it hlmt_materials_disc:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use map::{FourCC, Map};

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn memory(m: &mut Map<Cursor<Vec<u8>>>) -> tags::schema::Memory {
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    tags::schema::Memory::from_regions(
        vec![(space.virtual_base, meta)],
        vec![(space.virtual_base, space.virtual_base + space.size)],
    )
}

/// One `hlmt`'s materials and what its physics and collision models use.
#[derive(Debug)]
pub struct HlmtMaterials {
    pub name: String,
    /// `+0x58` elements: name, `+4`, `+8`, `+0xc` name, `+0x10` `i16`.
    pub materials: Vec<(String, u32, u32, String, i16, u32)>,
    /// The `phmo`'s materials (`+0x40`, `0xc`, a name first), if any.
    pub phmo: Option<Vec<String>>,
    /// The `phmo` materials' `+4` names and `+8` words.
    pub phmo_words: Vec<(String, u32)>,
    /// The largest material index a `phmo` shape names (shape header
    /// `+4`), over spheres, pills, boxes, triangles and polyhedra.
    pub phmo_max_shape_material: Option<u16>,
    /// The `coll`'s materials (`+0x14`, 4 each), if any.
    pub coll: Option<Vec<String>>,
}

/// Every `hlmt` of `m`.
pub fn survey(m: &mut Map<Cursor<Vec<u8>>>) -> (Vec<HlmtMaterials>, Vec<String>) {
    let mem = memory(m);
    let w = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    };
    let strings = m.strings().to_vec();
    let s = |v: u32| {
        if v == 0 {
            return String::new();
        }
        strings
            .get((v & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("{v:#x}"))
    };
    let tags = m.tags().to_vec();
    let addr = |id: u32| {
        tags.iter()
            .find(|t| t.id.0 == id && id != u32::MAX)
            .and_then(|t| t.location.map(|l| l.address))
    };
    let globals: Vec<String> = tags
        .iter()
        .find(|t| t.group == FourCC(*b"matg"))
        .and_then(|t| t.location)
        .map(|l| {
            let (n, first) = (w(l.address + 0x150), w(l.address + 0x154));
            (0..n).map(|i| s(w(first + i * 0xb4))).collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for t in tags.iter().filter(|t| t.group == FourCC(*b"hlmt")) {
        let Some(l) = t.location else { continue };
        let a = l.address;
        let (n, first) = (w(a + 0x58), w(a + 0x5c));
        let materials = (0..n.min(256))
            .map(|i| {
                let e = first + i * 0x14;
                (
                    s(w(e)),
                    w(e + 4),
                    w(e + 8),
                    s(w(e + 0xc)),
                    w(e + 0x10) as u16 as i16,
                    w(e + 0x10) >> 16,
                )
            })
            .collect();
        let phmo_at = addr(w(a + 0x24));
        let phmo = phmo_at.map(|p| {
            let (n, first) = (w(p + 0x40), w(p + 0x44));
            (0..n.min(256)).map(|i| s(w(first + i * 0xc))).collect()
        });
        // Shape blocks of the phmo root (defs/phmo.txt): spheres
        // 0x48, pills 0x58, boxes 0x60, triangles 0x68, polyhedra 0x70,
        // with their element sizes.
        let phmo_words = phmo_at
            .map(|p| {
                let (n, first) = (w(p + 0x40), w(p + 0x44));
                (0..n.min(256))
                    .map(|i| (s(w(first + i * 0xc + 4)), w(first + i * 0xc + 8)))
                    .collect()
            })
            .unwrap_or_default();
        let phmo_max_shape_material = phmo_at.and_then(|p| {
            let mut max = None::<u16>;
            for (field, size) in [
                (0x48u32, 0x80u32),
                (0x58, 0x50),
                (0x60, 0x90),
                (0x68, 0x60),
                (0x70, 0x100),
            ] {
                let (n, first) = (w(p + field), w(p + field + 4));
                for i in 0..n.min(512) {
                    let m = w(first + i * size + 4) as u16;
                    max = Some(max.map_or(m, |x: u16| x.max(m)));
                }
            }
            max
        });
        let coll = addr(w(a + 0xc)).map(|c| {
            let (n, first) = (w(c + 0x14), w(c + 0x18));
            (0..n.min(256)).map(|i| s(w(first + i * 4))).collect()
        });
        out.push(HlmtMaterials {
            name: t.name.clone().unwrap_or_default(),
            materials,
            phmo,
            phmo_words,
            phmo_max_shape_material,
            coll,
        });
    }
    (out, globals)
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_hlmt_materials() {
    for name in ["coagulation", "beavercreek", "05a_deltaapproach"] {
        let mut m = h2_map(name);
        let (all, globals) = survey(&mut m);
        let mut stats = std::collections::BTreeMap::<String, usize>::new();
        let mut bump = |k: &str| *stats.entry(k.into()).or_default() += 1;
        for h in &all {
            bump("hlmt");
            if h.materials.is_empty() {
                bump("empty block_58");
                if h.phmo.is_some() {
                    bump("empty block_58 with a phmo");
                    println!("{name}: {} has a phmo and no materials", h.name);
                }
            }
            if let Some(p) = &h.phmo {
                bump("with phmo");
                if p.len() <= h.materials.len() {
                    bump("phmo materials <= block_58");
                }
                if p.iter().zip(&h.materials).all(|(a, b)| *a == b.0) {
                    bump("phmo material names match block_58 names in order");
                }
                if let Some(mx) = h.phmo_max_shape_material {
                    if (mx as usize) < h.materials.len() {
                        bump("max shape material < block_58");
                    } else {
                        println!(
                            "{name}: {} shape material {mx} of {}",
                            h.name,
                            h.materials.len()
                        );
                    }
                }
            }
            if let Some(c) = &h.coll {
                bump("with coll");
                if c.len() <= h.materials.len() {
                    bump("coll materials <= block_58");
                }
                if c.iter().zip(&h.materials).all(|(a, b)| *a == b.0) {
                    bump("coll material names match block_58 names in order");
                }
            }
            if let Some(p) = &h.phmo {
                for (j, (g, x)) in h.phmo_words.iter().enumerate() {
                    let i = x & 0xffff;
                    let same = h.materials.get(i as usize).map(|m| m.0 == p[j]);
                    bump(&format!(
                        "phmo +8 high {:#x}, low indexes a block_58 element of the same name: {same:?}",
                        x >> 16
                    ));
                    let gm = globals.iter().position(|q| q == g);
                    bump(&format!("phmo +4 is a global material: {}", gm.is_some()));
                }
            }
            for (k, m) in h.materials.iter().enumerate() {
                bump(&format!("+0x10 high {:#x}", m.5));
                bump(&format!("+4 {:#x}", m.1));
                let k8 = m.2 as u16 as usize == k;
                bump(&format!("+8 low is k: {k8}, high {:#x}", m.2 >> 16));
                let key = if m.3.is_empty() { &m.0 } else { &m.3 };
                if globals.iter().position(|g| g == key).map(|g| g as i16) == Some(m.4) {
                    bump("+0x10 is the global index of +0xc, else of +0");
                }
                bump("material");
                let g = globals.iter().position(|g| *g == m.3);
                if g.map(|g| g as i16) == Some(m.4) {
                    bump("+0x10 is the index of the +0xc name in the globals");
                } else {
                    println!("{name}: {} material {:?} global {g:?}", h.name, m);
                }
            }
        }
        println!("{name}: {stats:#?}");
        for h in all.iter().take(6) {
            println!("{name}: {h:?}");
        }
    }
}

/// Halo 1 collision materials (`coll +0x234`, `0x48` each): the name and
/// the `u16`s at `+0x20`..`+0x28`, on Blood Gulch.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn h1_collision_material_types() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map("bloodgulch").unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let space = h1.tag_space();
    for t in h1.tags().iter().filter(|t| t.group.to_string() == "coll") {
        let b = h1.tag_data(t).unwrap();
        for m in space.block_at(b, 0x234, 0x48).unwrap() {
            let end = m[..32].iter().position(|c| *c == 0).unwrap_or(32);
            let u = |o: usize| u16::from_le_bytes([m[o], m[o + 1]]);
            println!(
                "{}: {:?} +20 {:#x} +24 {} +26 {}",
                t.name,
                String::from_utf8_lossy(&m[..end]),
                u32::from_le_bytes(m[0x20..0x24].try_into().unwrap()),
                u(0x24),
                u(0x26)
            );
        }
    }
}
