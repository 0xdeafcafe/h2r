//! Which render fields of a structure BSP's clusters, parts and sections
//! retail always fills, and whether converted structures fill them too.
//!
//! Reads the owner's Halo 2 disc (`$H2_DISC`) and, for the converted side,
//! the Halo 1 disc (`$H1_ISO`). `H1_MAPS=a,b` limits the converted maps.
//! Only which 16-bit words are nonzero is compared; no retail bytes are
//! kept or written.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::{MAPS, Sandbox, convert, known};
use h1map::H1Map;
use map::{CacheType, FourCC, Map};
use tags::structure::StructureRender;

struct Mem(Vec<u8>);
impl tags::geometry::RawSource for Mem {
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let s = p.offset as usize;
        self.0
            .get(s..s + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::Invalid("raw range"))
    }
}

/// A record kind whose 16-bit words are counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Record {
    Cluster,
    Part,
    Section,
}

/// Per 16-bit word of a record: how often it was nonzero, of how many.
#[derive(Default, Debug)]
struct Words {
    nonzero: Vec<usize>,
    seen: usize,
}
impl Words {
    fn add(&mut self, b: &[u8]) {
        if self.nonzero.len() < b.len() / 2 {
            self.nonzero.resize(b.len() / 2, 0);
        }
        for (i, w) in b.as_chunks::<2>().0.iter().enumerate() {
            if *w != [0, 0] {
                self.nonzero[i] += 1;
            }
        }
        self.seen += 1;
    }
    /// Byte offsets of the words nonzero in every record seen.
    fn always(&self) -> Vec<usize> {
        (0..self.nonzero.len())
            .filter(|&i| self.seen > 0 && self.nonzero[i] == self.seen)
            .map(|i| 2 * i)
            .collect()
    }
}

/// Words the comparison skips: the cluster's geometry block info
/// (+0x28..+0x54, checked by the decoders) and bounds (+0x54..+0x6c), and
/// the part's sphere centre (+0x10) and box (+0x30), coordinates that can
/// be exactly 0.0 in either game. A part's centre and box are checked
/// whole instead ([`Survey::empty_spheres`]).
fn compared(record: Record, at: usize) -> bool {
    match record {
        Record::Cluster => !(0x28..0x6c).contains(&at),
        Record::Part => !(0x10..0x2c).contains(&at) && !(0x30..0x48).contains(&at),
        Record::Section => true,
    }
}

/// Fields retail always fills that a converted structure leaves empty, and
/// why. Everything else retail always fills, every converted map must.
const NOT_CONVERTED: &[(Record, usize, &str)] = &[
    // Cluster portals: Halo 1's are converted (`h1convert::clusters`), but
    // a level that is one cluster (Hang 'Em High, Longest, Prisoner) or
    // sealed rooms (Chiron TL-34) has none, where every retail cluster has
    // some.
    (Record::Cluster, 0x8c, "portal count"),
    (Record::Cluster, 0x90, "portal pointer"),
    (Record::Cluster, 0x92, "portal pointer"),
];

#[derive(Default, Debug)]
struct Survey {
    cluster: Words,
    part: Words,
    section: Words,
    /// Stream types and how many sections carry each.
    streams: std::collections::BTreeMap<u8, usize>,
    sections: usize,
    /// Parts whose centre and box are all zero.
    empty_spheres: usize,
    /// Per 32-bit word of a bitmap image (0x74 bytes): how often it is
    /// `0xffffffff` ("none": no LOD, not in the texture cache), and of how
    /// many images.
    image_none: Vec<usize>,
    images: usize,
    /// Sections whose visibility code and reorder table the engine's
    /// volume pass would misread, and why.
    visibility: Vec<String>,
}

/// Bitmap image size.
const IMAGE: usize = 0x74;

/// Counts which words of every image of every bitmap in `m` are none.
fn survey_images(m: &mut Map<Cursor<Vec<u8>>>, s: &mut Survey) {
    s.image_none.resize(IMAGE / 4, 0);
    let ids: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm"))
        .filter_map(|t| Some((t.id, t.location?.address)))
        .collect();
    for (id, at) in ids {
        let b = m.read_tag_bytes(id).unwrap();
        let word = |o: usize| {
            b.get(o..o + 4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
        };
        let (Some(n), Some(p)) = (word(0x44), word(0x48)) else {
            continue;
        };
        let Some(p) = p.checked_sub(at).map(|p| p as usize) else {
            continue;
        };
        for i in 0..n as usize {
            let o = p + IMAGE * i;
            if o + IMAGE > b.len() {
                break;
            }
            for k in 0..IMAGE / 4 {
                if word(o + 4 * k) == Some(u32::MAX) {
                    s.image_none[k] += 1;
                }
            }
            s.images += 1;
        }
    }
}
impl Survey {
    fn words(&self, r: Record) -> &Words {
        match r {
            Record::Cluster => &self.cluster,
            Record::Part => &self.part,
            Record::Section => &self.section,
        }
    }
}

/// Every drawn cluster's records of one structure BSP in `m`.
fn survey_bsp(m: &mut Map<Cursor<Vec<u8>>>, file: &[u8], id: map::TagId, s: &mut Survey) {
    let render = StructureRender::read(m, id).unwrap();
    let mut raw = Mem(file.to_vec());
    for c in &render.clusters {
        let Some(sec) = c
            .geometry
            .load(&mut raw, Some(render.material_count as usize))
            .unwrap()
        else {
            continue;
        };
        if sec.parts.is_empty() {
            continue;
        }
        s.cluster.add(&c.raw);
        for p in &sec.parts {
            s.part.add(&p.raw);
            if p.raw[0x10..0x1c]
                .iter()
                .chain(&p.raw[0x30..0x48])
                .all(|&b| b == 0)
            {
                s.empty_spheres += 1;
            }
        }
        let block = tags::geometry::load_block(&c.geometry.block, &mut raw)
            .unwrap()
            .unwrap();
        s.section.add(block.section());
        if let Err(e) = visibility_layout(&block) {
            s.visibility.push(e);
        }
        for v in &sec.vertex_buffers {
            *s.streams.entry(v.type_id).or_default() += 1;
        }
        s.sections += 1;
    }
}

fn retail_survey(disc: &str) -> Survey {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc).unwrap())).unwrap();
    let mut s = Survey::default();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        let Ok(mut m) = Map::from_reader(Cursor::new(bytes.clone())) else {
            continue;
        };
        if m.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            survey_bsp(&mut m, &bytes, id, &mut s);
        }
        survey_images(&mut m, &mut s);
    }
    s
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_render_fields() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let s = retail_survey(&disc);
    for r in [Record::Cluster, Record::Part, Record::Section] {
        let w = s.words(r);
        eprintln!("{r:?}: {} records", w.seen);
        for (i, n) in w.nonzero.iter().enumerate() {
            if *n > 0 {
                eprintln!("  +{:#04x}: {n}", 2 * i);
            }
        }
    }
    eprintln!("streams over {} sections: {:?}", s.sections, s.streams);
    assert!(s.cluster.seen > 0);
}

/// Every converted multiplayer map's structure, surveyed.
fn converted_survey(h1_iso: &str, h2_iso: &str) -> (Survey, Vec<String>) {
    let only: Option<Vec<String>> = std::env::var("H1_MAPS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut h2 = xiso::Xiso::open(BufReader::new(File::open(h2_iso).unwrap())).unwrap();
    let h2_entries = h2.entries().unwrap();
    let mut s = Survey::default();
    let mut names = Vec::new();
    for (name, _, _) in MAPS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|m| m == name)) {
            continue;
        }
        let Some(e) = disc.map(name).cloned() else {
            continue;
        };
        let file = iso.read(&e).unwrap();
        let sha = content::Sha256::of(&file);
        let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
        let base_name = known(name).unwrap().1;
        let base = h2_entries
            .iter()
            .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{base_name}.map")))
            .unwrap();
        let mut base = Map::from_reader(Cursor::new(h2.read(base).unwrap())).unwrap();
        let done = convert(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        let sbsp: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in sbsp {
            survey_bsp(&mut m, &done.bytes, id, &mut s);
        }
        survey_images(&mut m, &mut s);
        names.push((*name).to_owned());
    }
    (s, names)
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn converted_structures_fill_every_render_field_retail_always_fills() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_iso = std::env::var("H2_DISC").expect("set H2_DISC");
    let retail = retail_survey(&h2_iso);
    let (ours, names) = converted_survey(&h1_iso, &h2_iso);
    assert!(retail.cluster.seen > 0 && ours.cluster.seen > 0);
    eprintln!(
        "retail: {} clusters, {} parts; converted ({}): {} clusters, {} parts",
        retail.cluster.seen,
        retail.part.seen,
        names.join(", "),
        ours.cluster.seen,
        ours.part.seen
    );
    let mut missing = Vec::new();
    for r in [Record::Cluster, Record::Part, Record::Section] {
        let filled = ours.words(r).always();
        for at in retail.words(r).always() {
            if !compared(r, at) || filled.contains(&at) {
                continue;
            }
            if NOT_CONVERTED.iter().any(|&(k, o, _)| k == r && o == at) {
                eprintln!("  {r:?} +{at:#04x}: not converted (listed)");
                continue;
            }
            let n = ours.words(r).nonzero.get(at / 2).copied().unwrap_or(0);
            missing.push(format!("{r:?} +{at:#04x} ({n} of {})", ours.words(r).seen));
        }
    }
    // Every stream type retail sections always carry, ours carry too.
    for (&t, &n) in &retail.streams {
        if n == retail.sections {
            let have = ours.streams.get(&t).copied().unwrap_or(0);
            if have != ours.sections {
                missing.push(format!("stream {t:#04x} ({have} of {})", ours.sections));
            }
        }
    }
    // Image words none on every retail image (LOD pointers past the
    // first, texture cache handles) are none on every converted one: a
    // handle of 0 reads as "resident in cache slot 0" and the pixels are
    // never loaded.
    for (k, &n) in retail.image_none.iter().enumerate() {
        let have = ours.image_none.get(k).copied().unwrap_or(0);
        if n == retail.images && have != ours.images {
            missing.push(format!(
                "image +{:#04x} none ({have} of {})",
                4 * k,
                ours.images
            ));
        }
    }
    assert_eq!(
        retail.image_none[0x28 / 4],
        retail.images,
        "every retail texture cache handle is none"
    );
    assert_eq!(retail.empty_spheres, 0);
    for e in ours.visibility.iter().take(8) {
        missing.push(format!("visibility: {e}"));
    }
    if ours.empty_spheres > 0 {
        missing.push(format!(
            "{} parts with no centre or box",
            ours.empty_spheres
        ));
    }
    assert!(
        missing.is_empty(),
        "retail always fills, converted maps don't: {missing:#?}"
    );
}

/// Section `block`'s visibility code and reorder table, checked as the
/// engine's volume pass reads them (`h1convert::structure::check_visibility`).
fn visibility_layout(block: &tags::geometry::LoadedBlock) -> Result<(), String> {
    let sec = block.section();
    let n = u32::from_le_bytes(sec[8..12].try_into().unwrap()) as usize;
    let res = |field: u16| {
        block
            .resources()
            .iter()
            .find(|r| {
                r.field_offset == field && r.kind != tags::geometry::ResourceKind::VertexBuffer
            })
            .and_then(|r| block.resource(r))
    };
    let code = res(0x28).ok_or("no visibility code")?;
    let table = res(0x30).ok_or("no reorder table")?;
    h1convert::structure::check_visibility(n, code, table)
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_visibility_layout() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc).unwrap())).unwrap();
    let mut ok = 0;
    let mut bad = std::collections::BTreeMap::<String, usize>::new();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        let Ok(mut m) = Map::from_reader(Cursor::new(bytes.clone())) else {
            continue;
        };
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            let render = StructureRender::read(&mut m, id).unwrap();
            let mut raw = Mem(bytes.clone());
            for c in &render.clusters {
                let Ok(Some(block)) = tags::geometry::load_block(&c.geometry.block, &mut raw)
                else {
                    continue;
                };
                match visibility_layout(&block) {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        let e = e
                            .split(|c: char| c.is_ascii_digit())
                            .next()
                            .unwrap()
                            .to_owned();
                        *bad.entry(e).or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("retail: {ok} sections as the normal path reads them; otherwise {bad:#?}");
    assert!(ok > 0);
}
