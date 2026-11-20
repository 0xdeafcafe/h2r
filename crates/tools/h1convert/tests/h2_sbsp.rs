//! The Halo 2 side: where a structure BSP keeps its collision BSP. Reads the
//! owner's Halo 2 disc (`$H2_DISC`); `H2_MAPS` picks maps (comma-separated
//! stems, default every multiplayer map).

use std::fs::File;
use std::io::BufReader;

use map::{CacheType, FourCC, Map};
use tags::collision::{BSP_BLOCKS_SIZE, Bsp};

/// `sbsp+0x0c`: collision materials; `+0x14`: the collision BSP block.
const SBSP_COLLISION_MATERIALS: usize = 0x0c;
const SBSP_COLLISION_BSP: usize = 0x14;

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_structure_bsp_holds_one_collision_bsp_at_0x14() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC to the Halo 2 disc");
    let wanted: Option<Vec<String>> = std::env::var("H2_MAPS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let (mut checked, mut no_material) = (0, 0);
    for e in iso.entries().unwrap() {
        let Some(stem) = e
            .path
            .rsplit(['/', '\\'])
            .next()
            .and_then(|f| f.strip_suffix(".map"))
        else {
            continue;
        };
        if wanted
            .as_ref()
            .is_some_and(|w| !w.iter().any(|x| x == stem))
        {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        if wanted.is_none() && m.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let bsps: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| (t.id, t.location.unwrap().address))
            .collect();
        for (id, address) in bsps {
            let b = m.read_tag_bytes(id).unwrap();
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            assert_eq!(u(SBSP_COLLISION_BSP), 1, "{stem}: one collision BSP");
            let at = (u(SBSP_COLLISION_BSP + 4) - address) as usize;
            assert!(at + BSP_BLOCKS_SIZE <= b.len());
            let bsp = Bsp::decode_blocks(&b, address, at).unwrap();
            let materials = u(SBSP_COLLISION_MATERIALS) as usize;
            // Structure surfaces may have no material (0xffff), which the
            // object-collision validator doesn't allow; count those apart.
            let (none, bad): (Vec<_>, Vec<_>) = bsp
                .validate(materials)
                .into_iter()
                .partition(|p| p.ends_with("material 65535 out of range"));
            assert!(bad.is_empty(), "{stem}: {:?}", &bad[..bad.len().min(5)]);
            no_material += none.len();
            eprintln!(
                "{stem}: {} nodes, {} surfaces, {} edges, {} materials",
                bsp.nodes.len(),
                bsp.surfaces.len(),
                bsp.edges.len(),
                materials
            );
            checked += 1;
        }
    }
    eprintln!("{checked} structure BSPs; {no_material} surfaces with no material");
    assert!(checked > 0);
}

/// Every block field of the structure BSP root on the disc's multiplayer
/// maps: how many maps use it, and the element size the spacing allows
/// (the smallest gap to the next block over its count, across maps). Also
/// checks that the block at `+0x2c` runs parallel to the collision leaves.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn structure_bsp_root_blocks_on_the_disc() {
    use std::collections::BTreeMap;
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let mut fields: BTreeMap<usize, (usize, f64, u32)> = BTreeMap::new();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        if m.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let t = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .unwrap()
            .clone();
        let loc = t.location.unwrap();
        let b = m.read_tag_bytes(t.id).unwrap();
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let end = loc.address + loc.size;
        let mut rows = Vec::new();
        for o in (0..0x240).step_by(4) {
            let (c, a) = (u(o), u(o + 4));
            if c > 0
                && c < 4_000_000
                && a >= loc.address
                && a < end
                && (a - loc.address) as usize > o
            {
                rows.push((o, c, a));
            }
        }
        for &(o, c, a) in &rows {
            let next = rows
                .iter()
                .map(|r| r.2)
                .filter(|&x| x > a)
                .min()
                .unwrap_or(end);
            let span = (next - a) as f64 / c as f64;
            let f = fields.entry(o).or_insert((0, f64::MAX, 0));
            f.0 += 1;
            f.1 = f.1.min(span);
            f.2 = f.2.max(c);
        }
        let coll = (u(SBSP_COLLISION_BSP + 4) - loc.address) as usize;
        let leaves = u32::from_le_bytes(b[coll + 16..coll + 20].try_into().unwrap());
        assert_eq!(
            u(0x2c),
            leaves,
            "{}: +0x2c parallels the collision leaves",
            e.path
        );
    }
    for (o, (maps, span, max)) in &fields {
        eprintln!("+{o:#05x}: {maps:>2} maps, element <= {span:.1} bytes, up to {max}");
    }
}

/// Where a retail cluster's background sound and sound environment live:
/// cluster `+0x72` and `+0x74` (i16) index the structure's own palettes,
/// `sbsp +0xd4` (`0x64` each) and `+0xdc` (`0x48` each) (E `0x0011bf46`,
/// `0x0018d890`, `0x0018ddf0`), which hold the same tags as the
/// scenario's (`scnr +0x250`, `+0x258`).
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn clusters_name_the_structure_s_sound_palettes() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC to the Halo 2 disc");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let (mut clusters, mut named, mut bad, mut same, mut differ) = (0, [0; 2], 0, 0, 0);
    let mut no_geometry = 0;
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        let Ok(mut m) = Map::from_reader(std::io::Cursor::new(bytes)) else {
            continue;
        };
        let Some(scnr) = m.scenario().cloned() else {
            continue;
        };
        let Some(sloc) = scnr.location else { continue };
        let s = m.read_tag_bytes(scnr.id).unwrap();
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| (t.id, t.location.unwrap().address))
            .collect();
        for (id, at) in ids {
            if let Ok(r) = tags::structure::StructureRender::read(&mut m, id) {
                for c in &r.clusters {
                    if c.geometry.block.block.is_none() {
                        no_geometry += 1;
                    }
                }
            }
            let b = m.read_tag_bytes(id).unwrap();
            let w = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let block = |b: &[u8], base: u32, f: usize, len: usize| -> Vec<Vec<u8>> {
                let (n, p) = (w(b, f) as usize, w(b, f + 4));
                if n == 0 {
                    return Vec::new();
                }
                let p = p.wrapping_sub(base) as usize;
                (0..n)
                    .map(|k| b[p + k * len..p + (k + 1) * len].to_vec())
                    .collect()
            };
            let palettes = [block(&b, at, 0xd4, 0x64), block(&b, at, 0xdc, 0x48)];
            let scenario = [
                block(&s, sloc.address, 0x250, 0x64),
                block(&s, sloc.address, 0x258, 0x48),
            ];
            for k in 0..2 {
                let tags = |p: &[Vec<u8>]| p.iter().map(|e| w(e, 0x24)).collect::<Vec<_>>();
                if tags(&palettes[k]) == tags(&scenario[k]) {
                    same += 1;
                } else {
                    differ += 1;
                    eprintln!(
                        "{}: palette {k}: sbsp {:x?} scnr {:x?}",
                        e.path,
                        tags(&palettes[k]),
                        tags(&scenario[k])
                    );
                }
            }
            for c in block(&b, at, 0x9c, 0xb0) {
                clusters += 1;
                for (k, o) in [0x72, 0x74].into_iter().enumerate() {
                    let v = i16::from_le_bytes([c[o], c[o + 1]]);
                    if v >= 0 {
                        named[k] += 1;
                        if v as usize >= palettes[k].len() {
                            bad += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "{clusters} clusters: {named:?} name a background sound / environment, {bad} out of range; palettes same as the scenario's {same}, differ {differ}; {no_geometry} with no geometry"
    );
    assert!(clusters > 0 && bad == 0);
}
