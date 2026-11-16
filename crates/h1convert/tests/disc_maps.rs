//! Every Halo 1 multiplayer map on the Halo 1 disc (`$H1_ISO`), converted
//! onto its Halo 2 base from the Halo 2 disc (`$H2_DISC`), and the
//! converted map's bitmaps decoded against the base's. Nothing is written
//! but a scratch copy of each converted map in `$TMPDIR` (the bitmap
//! decoder reads files), removed afterwards.
//!
//! `H1_MAPS=a,b` limits the maps; `H1_NO_BITMAPS=1` skips the bitmap check.

use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};

use assets::{Region, ResourceFiles};
use h1convert::convert::{MAPS, Sandbox, convert, known, prepare};
use h1map::H1Map;
use map::{FourCC, Map, ResourceFile};
use tags::{DecodedTag, TagData};

struct H2Disc {
    path: PathBuf,
    iso: xiso::Xiso<BufReader<File>>,
}

impl H2Disc {
    fn open(path: &str) -> Self {
        let iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
        Self {
            path: path.into(),
            iso,
        }
    }
    fn entry(&mut self, name: &str) -> xiso::Entry {
        self.iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
            .unwrap_or_else(|| panic!("no {name}.map on the Halo 2 disc"))
    }
    fn region(&mut self, name: &str) -> Region {
        let e = self.entry(name);
        Region {
            path: self.path.clone(),
            base: self.iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
            length: e.size.into(),
        }
    }
    fn read(&mut self, name: &str) -> Vec<u8> {
        let e = self.entry(name);
        self.iso.read(&e).unwrap()
    }
    /// Resource files with `local` as the map's own and the disc's shared
    /// maps beside it.
    fn files(&mut self, local: Region) -> ResourceFiles {
        let mut f = ResourceFiles::new();
        f.set(ResourceFile::Local, local).unwrap();
        for (sel, n) in [
            (ResourceFile::MainMenu, "mainmenu"),
            (ResourceFile::Shared, "shared"),
            (ResourceFile::SinglePlayerShared, "single_player_shared"),
        ] {
            let r = self.region(n);
            f.set(sel, r).unwrap();
        }
        f
    }
}

/// Every image of every bitmap: (tag name, image, the decoded top level's
/// RGBA, or the decode error).
fn bitmaps<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
) -> Vec<(String, usize, Result<Vec<u8>, String>)> {
    let ids: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm") && t.location.is_some())
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut out = Vec::new();
    for (id, name) in ids {
        let Ok(d) = DecodedTag::read(map, id) else {
            continue;
        };
        let TagData::Bitmap(b) = d.data else { continue };
        for (i, im) in b.images.iter().enumerate() {
            let px = assets::decode_image(im, files)
                .map(|d| d.levels.into_iter().flat_map(|l| l.rgba).collect())
                .map_err(|e| e.to_string());
            out.push((name.clone(), i, px));
        }
    }
    out
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_multiplayer_map_converts_and_keeps_the_bases_bitmaps() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_iso = std::env::var("H2_DISC").expect("set H2_DISC");
    let only: Option<Vec<String>> = std::env::var("H1_MAPS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let check_bitmaps = std::env::var_os("H1_NO_BITMAPS").is_none();
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut h2 = H2Disc::open(&h2_iso);
    let scratch = Path::new(&std::env::var("TMPDIR").unwrap_or("/tmp".into()))
        .join(format!("h1convert-disc-maps-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();

    let mut failures = Vec::new();
    let mut seen = 0;
    for (name, _, _) in MAPS {
        if only.as_ref().is_some_and(|o| !o.iter().any(|m| m == name)) {
            continue;
        }
        let Some(e) = disc.map(name).cloned() else {
            continue;
        };
        seen += 1;
        let file = iso.read(&e).unwrap();
        let sha = content::Sha256::of(&file);
        let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
        let base_name = known(name).unwrap().1;
        let base_bytes = h2.read(base_name);
        let mut base = Map::from_reader(Cursor::new(base_bytes)).unwrap();
        let p = match prepare(&h1, sha, &mut base, Sandbox::Halo2) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{name}: FAILS to prepare: {e}");
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let tris: usize = p.sections.iter().map(|s| s.indices.len() / 3).sum();
        eprintln!(
            "{name} onto {base_name}: {} clusters, {tris} triangles, {} materials; \
             spawns {}, flags {}, items {}, vehicles {}; source spawns {}, flags {}, items {}, vehicles {}",
            p.sections.len(),
            p.materials.len(),
            p.applied.counts[0],
            p.applied.counts[1],
            p.applied.counts[2],
            p.applied.vehicles,
            p.source.spawns,
            p.source.flags,
            p.source.items,
            p.source.vehicles,
        );
        for (h1s, (_, b)) in p.materials.iter().zip(&p.material_shaders) {
            eprintln!("    shader {h1s} -> {b}");
        }
        for d in p.applied.unresolved.iter().chain(&p.dropped) {
            eprintln!("    not placed: {d}");
        }
        let done = match convert(&h1, sha, &mut base, Sandbox::Halo2) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{name}: FAILS to convert: {e}");
                failures.push(format!("{name}: {e}"));
                continue;
            }
        };
        let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        m.validate_checksum().unwrap();
        // The structure's shaders and their bitmaps, by name.
        let sbsp = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"sbsp"))
            .unwrap()
            .id;
        let shaders = scenario_edit::geometry::bsp_shaders(&mut m, sbsp).unwrap();
        let mut named: Vec<String> = Vec::new();
        for s in &shaders {
            let n = m
                .tag(*s)
                .ok()
                .and_then(|t| t.name.clone())
                .unwrap_or_default();
            let (template, bitmaps) =
                scenario_edit::scene::shader_bitmaps(&mut m, *s).unwrap_or_default();
            let b: Vec<String> = bitmaps
                .iter()
                .filter_map(|b| m.tag(*b).ok().and_then(|t| t.name.clone()))
                .collect();
            let line = format!("{n} ({template}): {b:?}");
            if !named.contains(&line) {
                named.push(line);
            }
        }
        for l in &named {
            eprintln!("    structure shader {l}");
        }
        if !check_bitmaps {
            continue;
        }

        // Every bitmap decodes to the same pixels as in the base.
        let path = scratch.join(format!("{}.map", done.name));
        std::fs::write(&path, &done.bytes).unwrap();
        let mut converted_files = h2.files(Region {
            path: path.clone(),
            base: 0,
            length: done.bytes.len() as u64,
        });
        let base_region = h2.region(base_name);
        let mut base_files = h2.files(base_region);
        let mut base_map = Map::from_reader(Cursor::new(h2.read(base_name))).unwrap();
        let want = bitmaps(&mut base_map, &mut base_files);
        let got = bitmaps(&mut m, &mut converted_files);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(got.len(), want.len(), "{name}: image count");
        let differ: Vec<_> = got
            .iter()
            .zip(&want)
            .filter(|(g, w)| g != w)
            .map(|(g, _)| format!("{} [{}]", g.0, g.1))
            .collect();
        let decoded = want.iter().filter(|w| w.2.is_ok()).count();
        eprintln!(
            "    bitmaps: {} images, {decoded} decode in the base, {} differ",
            want.len(),
            differ.len()
        );
        if !differ.is_empty() {
            failures.push(format!(
                "{name}: {} images differ from the base, first {:?}",
                differ.len(),
                &differ[..differ.len().min(5)]
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(seen > 0, "no Halo 1 multiplayer maps found on {h1_iso}");
    assert_eq!(failures, Vec::<String>::new());
}
