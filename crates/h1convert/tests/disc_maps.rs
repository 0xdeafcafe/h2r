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

/// What the editor needs to texture the converted structure, checked
/// without rendering: every drawn part's material names the shader
/// [`h1convert::shaders::pick`] chose for its Halo 1 shader; the chosen
/// shaders' bitmaps aren't laid out for one mesh (`unwrapped`); and each
/// cluster's texture coordinates are the converter's diffuse UVs (stream
/// `18`), its lightmap ones stream `1f`, both read back exactly.
fn check_structure_texturing(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    base: &mut Map<Cursor<Vec<u8>>>,
    bytes: &[u8],
    p: &h1convert::convert::Prepared,
) {
    use tags::structure::StructureRender;
    let sbsp = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"sbsp"))
        .unwrap()
        .id;
    let render = StructureRender::read(m, sbsp).unwrap();
    assert_eq!(render.material_count as usize, p.material_shaders.len());
    // The materials block: each material's shader datum at +0xc.
    let sb = m.read_tag_bytes(sbsp).unwrap();
    let at = m.tag(sbsp).unwrap().location.unwrap().address;
    let w = |o: usize| u32::from_le_bytes(sb[o..o + 4].try_into().unwrap());
    let mats = (w(0xa8) - at) as usize;
    let shader_of: Vec<String> = (0..p.material_shaders.len())
        .map(|k| {
            let id = map::TagId(w(mats + 0x20 * k + 0xc));
            m.tag(id).unwrap().name.clone().unwrap()
        })
        .collect();
    let chosen: Vec<&String> = p.material_shaders.iter().map(|(_, n)| n).collect();
    assert_eq!(shader_of.iter().collect::<Vec<_>>(), chosen, "{name}");
    for n in &shader_of {
        let id = m
            .tags()
            .iter()
            .find(|t| t.name.as_deref() == Some(n.as_str()))
            .unwrap()
            .id;
        // The shader reads as it does in the base (the reader doesn't
        // follow every shader; one it can't read in the base is skipped).
        let got = scenario_edit::scene::shader_bitmaps(m, id).map_err(|e| e.to_string());
        let want = scenario_edit::scene::shader_bitmaps(base, id).map_err(|e| e.to_string());
        assert_eq!(got.is_ok(), want.is_ok(), "{name}: shader {n}");
        let Ok((_, bitmaps)) = got else {
            eprintln!("    (shader {n} unread: {})", want.unwrap_err());
            continue;
        };
        assert_eq!(Ok(bitmaps.clone()), want.map(|w| w.1), "{name}: shader {n}");
        for b in bitmaps {
            let bn = m.tag(b).unwrap().name.clone().unwrap_or_default();
            assert!(!bn.contains("unwrap"), "{name}: {n} uses {bn}");
        }
    }
    let mut raw = Mem(bytes.to_vec());
    for (c, s) in render.clusters.iter().zip(&p.sections) {
        let got = c
            .geometry
            .load(&mut raw, Some(render.material_count as usize))
            .unwrap()
            .unwrap();
        let v = got.vertices(None).unwrap();
        let uv: Vec<[f32; 2]> = s.vertices.iter().map(|v| v.texcoord).collect();
        assert_eq!(v.texcoords, uv, "{name}: diffuse UVs");
        for (a, b) in v.lightmap_texcoords.iter().zip(&s.vertices) {
            for k in 0..2 {
                assert!((a[k] - b.lightmap[k]).abs() < 1e-4, "{name}: lightmap UVs");
            }
        }
        let parts: Vec<u16> = got.parts.iter().map(|p| p.material).collect();
        let want: Vec<u16> = s.parts.iter().map(|p| p.material).collect();
        assert_eq!(parts, want, "{name}: part materials");
    }
    // Diffuse UVs tile (some leave [0, 1]); lightmap UVs never do.
    let all = p.sections.iter().flat_map(|s| &s.vertices);
    assert!(
        all.clone()
            .any(|v| v.texcoord.iter().any(|c| !(0.0..=1.0).contains(c)))
    );
    assert!(
        all.clone()
            .all(|v| v.lightmap.iter().all(|c| (0.0..=1.0).contains(c)))
    );
}

/// Every vehicle spawn of a converted map: the collection's role (last
/// name component), the vehicle its single entry spawns (last name
/// component), its variant name, and the netgame type. Asserts each
/// collection has one entry naming a `vehi`.
fn vehicle_spawns<R: std::io::Read + std::io::Seek>(
    m: &mut Map<R>,
) -> Vec<(String, String, String, i64)> {
    use scenario_edit::{BlockKind, Scenario, Value};
    let s = Scenario::read(m).unwrap();
    let e = BlockKind::NetgameEquipment;
    let last = |n: &str| n.rsplit('\\').next().unwrap().to_owned();
    let mut out = Vec::new();
    for i in 0..s.block(e).len() {
        let Value::Tag(r) = s.get(e, i, "item").unwrap() else {
            panic!()
        };
        if r.group != FourCC(*b"vehc") {
            continue;
        }
        let Value::Int(kind) = s.get(e, i, "type").unwrap() else {
            panic!()
        };
        let t = m.tag(r.id).unwrap().clone();
        let at = t.location.unwrap().address;
        let b = m.read_tag_bytes(r.id).unwrap();
        let w = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(w(0), 1, "{:?}: one entry", t.name);
        let entry = (w(4) - at) as usize;
        let ew = |o: usize| u32::from_le_bytes(b[entry + o..entry + o + 4].try_into().unwrap());
        assert_eq!(FourCC::from_u32(ew(4)), FourCC(*b"vehi"));
        let vehicle = m.tag(map::TagId(ew(8))).unwrap().name.clone().unwrap();
        let sid = ew(0xc);
        let variant = if sid == 0 {
            String::new()
        } else {
            m.strings()[(sid & 0xffffff) as usize]
                .clone()
                .unwrap_or_default()
        };
        out.push((
            last(t.name.as_deref().unwrap()),
            last(&vehicle),
            variant,
            kind,
        ));
    }
    out
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn coagulation_has_a_scorpion_but_no_collection_for_it() {
    let mut h2 = H2Disc::open(&std::env::var("H2_DISC").expect("set H2_DISC"));
    let m = Map::from_reader(Cursor::new(h2.read("coagulation"))).unwrap();
    let named = |g: &[u8; 4], n: &str| {
        m.tags()
            .iter()
            .any(|t| t.group.0 == *g && t.name.as_deref().is_some_and(|x| x.ends_with(n)))
    };
    assert!(named(b"vehi", "\\scorpion"));
    assert!(named(b"vehi", "\\warthog"));
    assert!(!named(b"vehc", "\\scorpion"));
    assert!(!named(b"vehc", "\\warthog_gauss"));
    // The variant names retail collections use, as string IDs: `gauss`
    // (index 0x694) and `multiplayer` (0x78b).
    assert_eq!(m.strings()[0x694].as_deref(), Some("gauss"));
    assert_eq!(m.strings()[0x78b].as_deref(), Some("multiplayer"));
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
        for c in &p.applied.collections {
            let was = base.tag(c.id).ok().and_then(|t| t.name.clone());
            eprintln!("    built {} in the slot of {was:?}", c.name);
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
        check_structure_texturing(name, &mut m, &mut base, &done.bytes, &p);
        // Every vehicle spawn names a collection whose entry is the right
        // vehicle and variant, with its class's netgame type.
        let vehicles = vehicle_spawns(&mut m);
        for (role, vehicle, variant, kind) in &vehicles {
            let class = h1convert::sandbox::vehicle_class(role).unwrap();
            assert_eq!(vehicle, class.vehicle, "{name}: {role}");
            if let Some(v) = class.variant {
                assert_eq!(variant, v, "{name}: {role}");
            }
            assert_eq!(*kind, i64::from(class.netgame_type), "{name}: {role}");
        }
        assert_eq!(vehicles.len(), p.applied.vehicles, "{name}");
        let mut roles: Vec<&str> = vehicles.iter().map(|v| v.0.as_str()).collect();
        roles.sort();
        eprintln!("    vehicles placed: {roles:?}");
        if name == "bloodgulch" {
            assert_eq!(p.applied.vehicles, p.source.vehicles, "every vehicle");
            // Xbox Blood Gulch has warthogs, ghosts and scorpions; no
            // rocket warthog (that came with the PC version).
            for r in ["scorpion", "warthog", "ghost"] {
                assert!(roles.contains(&r), "bloodgulch places a {r}");
            }
        }
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
