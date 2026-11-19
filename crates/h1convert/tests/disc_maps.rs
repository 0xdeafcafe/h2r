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

/// Tags the skies add: each sky and its model, and the shader copies,
/// lens flares and Halo 1 bitmaps (those the structure's textures don't
/// already add) of `m` past `base_slots` named for the skies' Halo 1
/// shaders, their lights' flares and the bitmaps both draw.
fn sky_tags(
    p: &h1convert::convert::Prepared,
    m: &Map<Cursor<Vec<u8>>>,
    base_slots: usize,
) -> usize {
    let skies: usize = p
        .skies
        .skies
        .iter()
        .map(|s| 1 + usize::from(s.model.is_some()))
        .sum();
    let shaders: Vec<&str> = p
        .skies
        .skies
        .iter()
        .flat_map(|s| s.shaders.iter().flatten().map(|h| h.name.as_str()))
        .collect();
    // Each sky light's lens flare and its bitmap (`Sky::install`).
    let flares = || {
        p.skies
            .skies
            .iter()
            .flat_map(|s| s.flares.iter().flatten())
    };
    let lenses: Vec<&str> = flares().map(|f| f.name.as_str()).collect();
    let bitmaps: Vec<&str> = p
        .skies
        .skies
        .iter()
        .flat_map(|s| s.bitmaps.iter().map(|b| b.1.as_str()))
        .chain(flares().filter_map(|f| f.bitmap.as_ref().map(|b| b.0.as_str())))
        .filter(|n| !p.textures.bitmaps.iter().any(|t| t.0 == *n))
        .collect();
    let added = m
        .tags()
        .iter()
        .filter(|t| t.id.index() >= base_slots)
        .filter(|t| {
            let n = t.name.as_deref().unwrap_or_default();
            (t.group == FourCC(*b"shad") && shaders.contains(&n))
                || (t.group == FourCC(*b"bitm") && bitmaps.contains(&n))
                || (t.group == FourCC(*b"lens") && lenses.contains(&n))
        })
        .count();
    // Opaque copies of the dome pictures drawn without their own alpha
    // (`H1SkyShader::opaque_picture`): exactly one per such picture, named
    // `opaque_name` of it, and no other added bitmap so named.
    let want: std::collections::BTreeSet<String> = p
        .skies
        .skies
        .iter()
        .flat_map(|s| {
            s.shaders.iter().flatten().filter_map(|h| {
                let id = h.maps[h.opaque_picture()?].bitmap;
                let b = s.bitmaps.iter().find(|b| b.0 == id)?;
                Some(h1convert::sky::opaque_name(&b.1))
            })
        })
        .collect();
    let opaque: Vec<String> = m
        .tags()
        .iter()
        .filter(|t| t.id.index() >= base_slots && t.group == FourCC(*b"bitm"))
        .filter_map(|t| t.name.clone())
        .filter(|n| n.ends_with(" opaque"))
        .collect();
    assert_eq!(
        opaque
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        want,
        "the opaque sky pictures"
    );
    assert_eq!(opaque.len(), want.len(), "one copy each: {opaque:?}");
    if !opaque.is_empty() {
        eprintln!("    opaque sky pictures: {opaque:?}");
    }
    // Each sky light's lens flare and its bitmap (`h1convert::lens`), where
    // added and not already counted as a sky or level bitmap.
    let flares: Vec<(&str, Option<&str>)> = p
        .skies
        .skies
        .iter()
        .flat_map(|s| s.flares.iter().flatten())
        .map(|f| (f.name.as_str(), f.bitmap.as_ref().map(|b| b.0.as_str())))
        .collect();
    let lens: Vec<String> = m
        .tags()
        .iter()
        .filter(|t| t.id.index() >= base_slots)
        .filter(|t| {
            let n = t.name.as_deref().unwrap_or_default();
            (t.group == FourCC(*b"lens") && flares.iter().any(|f| f.0 == n))
                || (t.group == FourCC(*b"bitm")
                    && flares.iter().any(|f| f.1 == Some(n))
                    && !bitmaps.contains(&n)
                    && !p.textures.bitmaps.iter().any(|b| b.0 == n))
        })
        .filter_map(|t| t.name.clone())
        .collect();
    if !lens.is_empty() {
        eprintln!("    sky flares: {lens:?}");
    }
    skies + added + opaque.len() + lens.len()
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
    // A material whose Halo 1 shader got a copy with Halo 1's textures
    // names the copy (named after the Halo 1 shader); the rest name the
    // chosen base shader.
    let copy_of = |k: usize| {
        p.textures
            .copies
            .iter()
            .find(|c| c.h1_shader == p.materials[k] && c.material == p.material_shaders[k].0)
    };
    let chosen: Vec<&String> = (0..p.material_shaders.len())
        .map(|k| copy_of(k).map_or(&p.material_shaders[k].1, |c| &c.h1_shader))
        .collect();
    assert_eq!(shader_of.iter().collect::<Vec<_>>(), chosen, "{name}");
    // Each part's kind is retail's for its material's template
    // (`structure::part_kind`): transparent shaders' parts are kind 4.
    let kind_of: Vec<u16> = (0..p.material_shaders.len())
        .map(|k| {
            let id = map::TagId(w(mats + 0x20 * k + 0xc));
            let t = scenario_edit::scene::shader_bitmaps(m, id)
                .map(|x| x.0)
                .unwrap_or_default();
            h1convert::structure::part_kind(&t)
        })
        .collect();
    let mut kinds = [0usize; 6];
    let mut raw = Mem(bytes.to_vec());
    for c in &render.clusters {
        let Some(s) = c.geometry.load(&mut raw, None).unwrap() else {
            continue;
        };
        for part in &s.parts {
            let want = kind_of[usize::from(part.material)];
            assert_eq!(
                part.kind,
                want,
                "{name}: material {} ({})",
                part.material,
                shader_of[usize::from(part.material)]
            );
            kinds[usize::from(part.kind).min(5)] += 1;
        }
    }
    eprintln!(
        "    part kinds 2/3/4: {}/{}/{}",
        kinds[2], kinds[3], kinds[4]
    );
    for (k, n) in shader_of.iter().enumerate() {
        let id = m
            .tags()
            .iter()
            .find(|t| t.name.as_deref() == Some(n.as_str()))
            .unwrap()
            .id;
        if let Some(c) = copy_of(k) {
            check_shader_copy(name, m, base, id, c);
            continue;
        }
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
        if s.indices.is_empty() {
            // A Halo 1 cluster with no surfaces: an empty section.
            continue;
        }
        let got = c
            .geometry
            .load(&mut raw, Some(render.material_count as usize))
            .unwrap()
            .unwrap();
        let v = got.vertices(None).unwrap();
        let uv: Vec<[f32; 2]> = s.vertices.iter().map(|v| v.texcoord).collect();
        assert_eq!(v.texcoords, uv, "{name}: diffuse UVs");
        for (a, b) in v.lightmap_texcoords.iter().zip(&s.vertices) {
            for (x, y) in a.iter().zip(b.lightmap) {
                assert!((x - y).abs() < 1e-4, "{name}: lightmap UVs");
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

/// A rebuilt shader with Halo 1's textures: it reads as a shader, its
/// template is its layout's stem, it draws every Halo 1 bitmap it was
/// given (named as in Halo 1), and resolved on the converted map, each
/// tiled map's scale reaches a vertex constant `c[18 + stage]`.
fn check_shader_copy(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    _base: &mut Map<Cursor<Vec<u8>>>,
    id: map::TagId,
    c: &h1convert::textures::ShaderCopy,
) {
    use tags::material::ShaderLibrary;
    let n = &c.h1_shader;
    let (template, got) = scenario_edit::scene::shader_bitmaps(m, id)
        .unwrap_or_else(|e| panic!("{name}: shader {n}: {e}"));
    assert_eq!(template, c.layout.stem, "{name}: {n}'s stem");
    let names: Vec<String> = got
        .iter()
        .filter_map(|b| m.tag(*b).ok().and_then(|t| t.name.clone()))
        .collect();
    for (_, h1) in &c.swapped {
        assert!(names.contains(h1), "{name}: {n} lacks {h1}");
    }
    let mut lib = ShaderLibrary::read(m).unwrap();
    let material = lib
        .material(id, None)
        .unwrap_or_else(|e| panic!("{name}: {n}: {e}"));
    let constants: Vec<[f32; 2]> = material
        .layers
        .iter()
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
        .flat_map(|i| &i.vertex_constants)
        .filter(|c| (18..22).contains(&c.index))
        // A stage's tiling is a constant's first half, or (the micro
        // detail of `tex_bump_detail_blend_detail`) its second.
        .flat_map(|c| [[c.value[0], c.value[1]], [c.value[2], c.value[3]]])
        .collect();
    if c.layout.stem.contains("\\opaque\\tex_bump") {
        for &(_, _, scale, _) in &c.maps {
            assert!(
                constants.contains(&scale),
                "{name}: {n}: no stage tiles {scale:?}"
            );
        }
    }
}

/// Halo 1's own textures: every bitmap the converter added decodes
/// (through Halo 2's decoder, from the converted map's own raw data) to
/// the pixels `h1bitmaps` decodes from the Halo 1 bitmap, every image's
/// top level. Returns (bitmaps, images).
fn check_h1_textures(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    files: &mut ResourceFiles,
    h1: &H1Map,
    p: &h1convert::convert::Prepared,
) -> (usize, usize) {
    let mut images = 0;
    for (bn, _) in &p.textures.bitmaps {
        let id = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"bitm") && t.name.as_deref() == Some(bn.as_str()))
            .unwrap_or_else(|| panic!("{name}: no bitmap {bn}"))
            .id;
        let d = DecodedTag::read(m, id).unwrap();
        let TagData::Bitmap(b) = d.data else {
            panic!("{name}: {bn} isn't a bitmap")
        };
        let tag = h1
            .tags()
            .iter()
            .find(|t| &t.name == bn && t.group.to_string() == "bitm")
            .unwrap()
            .clone();
        let src = h1map::bitmap::Bitmap::read(h1, &tag).unwrap();
        assert_eq!(b.images.len(), src.images.len(), "{name}: {bn}");
        for (i, im) in b.images.iter().enumerate() {
            let got = assets::decode_image(im, files)
                .unwrap_or_else(|e| panic!("{name}: {bn} [{i}]: {e}"));
            let want = h1bitmaps::decode::decode_image(&src.images[i], src.pixels(h1, i).unwrap())
                .unwrap();
            assert_eq!(got.levels[0].rgba, want[0].rgba, "{name}: {bn} [{i}]");
            images += 1;
        }
    }
    (p.textures.bitmaps.len(), images)
}

/// The level's own skies: every element of the scenario's skies block
/// names a `sky ` the conversion added from Halo 1 (none of the base's),
/// and every material of its model a copy of one of the base's sky
/// shaders named for its Halo 1 shader, drawing a bitmap the conversion
/// added. Returns the names.
fn check_skies(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    p: &h1convert::convert::Prepared,
    base_slots: usize,
) -> Vec<String> {
    let scnr = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"scnr"))
        .unwrap()
        .clone();
    let b = m.read_tag_bytes(scnr.id).unwrap();
    let at = scnr.location.unwrap().address;
    let w = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    for (o, what) in h1convert::convert::BASE_BLOCKS {
        assert_eq!(w(o), 0, "{name}: the base's {what} stay");
    }
    // Chiron TL34 (putput) and Rat Race are indoors: Halo 1 names no
    // sky, and the base's stays (nothing shows it).
    if p.skies.skies.is_empty() {
        assert!(p.skies.notes.is_empty(), "{name}: {:?}", p.skies.notes);
        return Vec::new();
    }
    let count = w(h1convert::sky::SKIES) as usize;
    assert!(count > 0, "{name}: the scenario has no sky");
    let start = (w(h1convert::sky::SKIES + 4) - at) as usize;
    let want: Vec<&str> = p.skies.skies.iter().map(|s| s.name.as_str()).collect();
    let mut names = Vec::new();
    for k in 0..count {
        let e = start + k * h1convert::sky::SKY;
        assert_eq!(w(e), FourCC(*b"sky ").as_u32(), "{name}: sky {k}");
        let t = m.tag(map::TagId(w(e + 4))).unwrap();
        assert_eq!(t.group, FourCC(*b"sky "), "{name}: sky {k}");
        let n = t.name.clone().unwrap_or_default();
        assert!(want.contains(&n.as_str()), "{name}: sky {k} is {n}");
        let sky = p.skies.skies.iter().find(|s| s.name == n).unwrap();
        check_sky_materials(name, m, map::TagId(w(e + 4)), sky, base_slots);
        names.push(n);
    }
    names
}

/// [`check_skies`]' materials check for the added sky `id`.
fn check_sky_materials(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    id: map::TagId,
    sky: &h1convert::sky::Sky,
    base_slots: usize,
) {
    use h1convert::sky::{MATERIAL_SHADER, MODE_MATERIAL, MODE_MATERIALS};
    use h1convert::sky_shaders::{ADDITIVE_TEMPLATE, ALPHA_TEMPLATE, CLOUDS_TEMPLATE};
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let model = map::TagId(word(&m.read_tag_bytes(id).unwrap(), 4));
    let Ok(t) = m.tag(model).cloned() else {
        assert!(sky.model.is_none(), "{name}: {}'s model went in", sky.name);
        return;
    };
    let b = m.read_tag_bytes(model).unwrap();
    let at = t.location.unwrap().address;
    let (count, start) = (
        word(&b, MODE_MATERIALS) as usize,
        word(&b, MODE_MATERIALS + 4),
    );
    assert_eq!(count, sky.shaders.len(), "{name}: {}", sky.name);
    for k in 0..count {
        let e = (start - at) as usize + k * MODE_MATERIAL;
        let shader = map::TagId(word(&b, e + MATERIAL_SHADER + 4));
        let h1 = sky.shaders[k]
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: {} material {k} reads", sky.name));
        let tag = m.tag(shader).unwrap().clone();
        assert_eq!(
            tag.name.as_deref(),
            Some(h1.name.as_str()),
            "{name}: material {k}"
        );
        let (template, bitmaps) = scenario_edit::scene::shader_bitmaps(m, shader).unwrap();
        assert!(
            [ALPHA_TEMPLATE, CLOUDS_TEMPLATE, ADDITIVE_TEMPLATE].contains(&template.as_str()),
            "{name}: {} is a {template}",
            h1.name
        );
        assert!(
            bitmaps.iter().any(|b| b.index() >= base_slots),
            "{name}: {} draws no Halo 1 bitmap",
            h1.name
        );
        eprintln!("      {} -> {template}", h1.name);
    }
}

/// The tags the level's ambience adds: each looping sound, each sound
/// they play (once), each sound environment. None has a base tag's path.
fn ambience_tags(p: &h1convert::convert::Prepared) -> usize {
    let mut sounds: Vec<&str> = Vec::new();
    for (_, l) in &p.ambience.backgrounds {
        for (n, _) in &l.sounds {
            if !sounds.contains(&n.as_str()) {
                sounds.push(n);
            }
        }
    }
    p.ambience.backgrounds.len() + sounds.len() + p.ambience.environments.len()
}

/// The level's own background sounds and sound environments: the
/// scenario's palettes name Halo 1's entries, each an `lsnd` or `snde`
/// the conversion added under Halo 1's path, and each looping sound's
/// tracks and details name `snd!` tags of Halo 1's paths. Returns the
/// palettes' tag names.
fn check_ambience(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    p: &h1convert::convert::Prepared,
) -> (Vec<String>, Vec<String>) {
    use h1convert::ambience::*;
    assert!(
        p.ambience.notes.is_empty(),
        "{name}: {:?}",
        p.ambience.notes
    );
    let scnr = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"scnr"))
        .unwrap()
        .clone();
    let b = m.read_tag_bytes(scnr.id).unwrap();
    let at = scnr.location.unwrap().address;
    let w = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut out = (Vec::new(), Vec::new());
    for (field, len, group, want, names) in [
        (
            BACKGROUND_SOUNDS,
            BACKGROUND_SOUND,
            *b"lsnd",
            p.ambience
                .backgrounds
                .iter()
                .map(|(_, l)| l.name.clone())
                .collect::<Vec<_>>(),
            &mut out.0,
        ),
        (
            SOUND_ENVIRONMENTS,
            SOUND_ENVIRONMENT,
            *b"snde",
            p.ambience
                .environments
                .iter()
                .map(|e| e.name.clone())
                .collect(),
            &mut out.1,
        ),
    ] {
        let count = w(&b, field) as usize;
        // As many as the base's palette holds.
        assert!(count <= want.len(), "{name}: {}", FourCC(group));
        assert!(count > 0 || want.is_empty(), "{name}: no {}", FourCC(group));
        let start = (w(&b, field + 4).wrapping_sub(at)) as usize;
        for k in 0..count {
            let e = start + k * len;
            assert_eq!(w(&b, e + 0x20), FourCC(group).as_u32(), "{name}");
            let t = m.tag(map::TagId(w(&b, e + 0x24))).unwrap().clone();
            assert_eq!(t.group, FourCC(group), "{name}");
            let n = t.name.clone().unwrap_or_default();
            assert!(want.contains(&n), "{name}: {} {k} is {n}", FourCC(group));
            names.push(n);
        }
    }
    for (_, l) in &p.ambience.backgrounds {
        let id = m
            .tag(map::TagQuery::NameInGroup(&l.name, FourCC(*b"lsnd")))
            .unwrap()
            .id;
        let lb = m.read_tag_bytes(id).unwrap();
        let tracks = w(&lb, 0x1c) as usize;
        assert_eq!(tracks, l.tracks.len(), "{name}: {}", l.name);
        let la = m.tag(id).unwrap().location.unwrap().address;
        let first = (w(&lb, 0x20).wrapping_sub(la)) as usize;
        for (k, t) in l.tracks.iter().enumerate() {
            let e = first + k * 0x58;
            let sound = &t.sounds[1];
            let d = w(&lb, e + 0x20);
            match sound {
                Some(s) => {
                    let got = m.tag(map::TagId(d)).unwrap();
                    assert_eq!(got.group, FourCC(*b"snd!"), "{name}: {}", l.name);
                    assert_eq!(got.name.as_deref(), Some(s.as_str()), "{name}");
                }
                None => assert_eq!(d, u32::MAX, "{name}: {}", l.name),
            }
        }
    }
    out
}

/// The converted lightmap: the lightmap names the converted bitmap, every
/// lit cluster's page is one of its images, every cluster's lightmap
/// coordinates lie in its page, and each page decodes (through Halo 2's
/// decoder, from the converted map's own raw data) to the pixels the
/// converter cut for its cluster. Returns the number of pages.
fn check_lightmap(
    name: &str,
    m: &mut Map<Cursor<Vec<u8>>>,
    files: &mut ResourceFiles,
    h1: &H1Map,
    p: &h1convert::convert::Prepared,
) -> usize {
    use tags::structure::{Lightmap, StructureRender};
    let l = p
        .lightmap
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: no lightmap"));
    let ltmp = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"ltmp"))
        .unwrap()
        .id;
    let sbsp = m
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"sbsp"))
        .unwrap()
        .id;
    let light = Lightmap::read(m, ltmp).unwrap();
    let g = &light.groups[0];
    assert_eq!(g.bitmap, Some(l.slot), "{name}: lightmap bitmap");
    let render = StructureRender::read(m, sbsp).unwrap();
    assert_eq!(g.cluster_render_info.len(), render.clusters.len());
    let d = DecodedTag::read(m, l.slot).unwrap();
    let TagData::Bitmap(b) = d.data else {
        panic!("{name}: the lightmap bitmap isn't a bitmap")
    };
    assert_eq!(b.images.len(), l.pages, "{name}: pages");
    let mut lit = 0;
    for (info, s) in g.cluster_render_info.iter().zip(&p.sections) {
        assert_eq!(info.palette_index, -1);
        match s.page {
            Some(page) => {
                lit += 1;
                assert_eq!(info.bitmap_index, page as i16, "{name}");
                assert!(usize::from(page) < b.images.len(), "{name}: page {page}");
            }
            None => assert_eq!(info.bitmap_index, -1, "{name}"),
        }
        for v in &s.vertices {
            assert!(v.lightmap.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }
    assert!(lit > 0, "{name}: no lit cluster");
    // Halo 1 names a level's lightmap bitmap after its scenario.
    let tag = h1
        .tags()
        .iter()
        .find(|t| t.name == l.source && t.group.to_string() == "bitm")
        .unwrap()
        .clone();
    // Each page is a lit cluster's, cut from Halo 1's pages
    // (`h1convert::clusters`; `tests/h1_clusters.rs` checks every lit
    // vertex samples the same light there): it decodes from the map as the
    // converter composed it, in Halo 1's page format.
    let src = h1map::bitmap::Bitmap::read(h1, &tag).unwrap();
    for (i, im) in b.images.iter().enumerate() {
        let got = assets::decode_image(im, files).unwrap();
        let mut record = src.images[0].clone();
        [record.width, record.height] = [im.dimensions[0], im.dimensions[1]];
        let want = h1bitmaps::decode::decode_image(&record, &l.bitmap.raw[i].0).unwrap();
        assert_eq!(
            got.levels[0].rgba, want[0].rgba,
            "{name}: lightmap page {i}"
        );
    }
    b.images.len()
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
        for c in &p.textures.copies {
            eprintln!(
                "    textures {} on {}: {:?}",
                c.h1_shader, c.layout.stem, c.swapped
            );
        }
        for n in &p.textures.notes {
            eprintln!("    textures: {n}");
        }
        if name == "bloodgulch" {
            assert!(
                p.textures.copies.len() >= 5,
                "{name}: Halo 1 textures on {} shaders",
                p.textures.copies.len()
            );
        }
        for c in &p.applied.collections {
            eprintln!("    built {} as {:?}", c.name, c.id);
        }
        let collections = p.applied.collections.clone();
        let base_slots = base.index_slots() as usize;
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
        // Built vehicle collections are added after the base's tags, at
        // the datums the placements name; no base tag gives up its slot.
        for c in &collections {
            let t = m.tag(c.id).unwrap();
            assert_eq!(t.group, FourCC(*b"vehc"), "{name}: {}", c.name);
            assert_eq!(t.name.as_deref(), Some(c.name.as_str()), "{name}");
            assert!(
                c.id.index() >= base_slots,
                "{name}: {} reuses a slot",
                c.name
            );
        }
        // Then Halo 1's textures: a bitmap per converted bitmap and a
        // shader per copy.
        let expected = base_slots
            + collections.len()
            + p.textures.bitmaps.len()
            + p.textures.copies.len()
            + sky_tags(&p, &m, base_slots)
            + ambience_tags(&p);
        if m.index_slots() as usize != expected {
            // What the conversion added, to see which part the count
            // misses: every added tag, and the added slots left empty.
            let added: Vec<String> = m
                .tags()
                .iter()
                .filter(|t| t.id.index() >= base_slots)
                .map(|t| format!("{} {}", t.group, t.name.as_deref().unwrap_or("?")))
                .collect();
            let live = m.tags().iter().filter(|t| t.id.index() >= base_slots).count();
            eprintln!(
                "{name}: {} slots from {base_slots}, {live} live: collections {}, texture bitmaps {}, copies {}, sky {}, ambience {}; added: {added:#?}",
                m.index_slots() as usize - base_slots,
                collections.len(),
                p.textures.bitmaps.len(),
                p.textures.copies.len(),
                sky_tags(&p, &m, base_slots),
                ambience_tags(&p),
            );
        }
        assert_eq!(m.index_slots() as usize, expected, "{name}");
        check_structure_texturing(name, &mut m, &mut base, &done.bytes, &p);
        let skies = check_skies(name, &mut m, &p, base_slots);
        eprintln!("    skies: {skies:?}");
        failures.extend(check_images(name, &mut m));
        let (sounds, environments) = check_ambience(name, &mut m, &p);
        eprintln!("    background sounds: {sounds:?}; sound environments: {environments:?}");
        // The engine compares every pair of clusters every tick: the
        // audibility element must cover them (the first Blood Gulch's crash).
        {
            let sbsp = m
                .tags()
                .iter()
                .find(|t| t.group == FourCC(*b"sbsp"))
                .unwrap()
                .clone();
            let bytes = m.read_tag_bytes(sbsp.id).unwrap();
            let at = sbsp.location.unwrap().address;
            if let Err(e) = h1convert::structure::check_audibility(&bytes, at) {
                failures.push(format!("{name}: audibility: {e}"));
            }
            if let Err(e) = h1convert::structure::check_sound_clusters(&bytes, at) {
                failures.push(format!("{name}: sound clusters: {e}"));
            }
        }
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
        let pages = check_lightmap(name, &mut m, &mut converted_files, &h1, &p);
        eprintln!(
            "    lightmap: {pages} pages from {}, in the slot of {}",
            p.lightmap.as_ref().unwrap().source,
            p.lightmap.as_ref().unwrap().replaces
        );
        let (h1_bitmaps, h1_images) =
            check_h1_textures(name, &mut m, &mut converted_files, &h1, &p);
        eprintln!(
            "    Halo 1 textures: {h1_bitmaps} bitmaps, {h1_images} images decode as in Halo 1"
        );
        let want = bitmaps(&mut base_map, &mut base_files);
        let got = bitmaps(&mut m, &mut converted_files);
        std::fs::remove_file(&path).unwrap();
        // The converted lightmap took the base's lightmap bitmap's slot:
        // every other image is the base's, less the base bitmaps nothing
        // in the converted map reaches, which pinned pruning drops
        // (`fresh::prune_pinned`): no tag of those names is left.
        let slot_name = p.lightmap.as_ref().map(|l| l.replaces.clone());
        let lightmap_name = slot_name.clone().unwrap_or_default();
        let present: std::collections::HashSet<String> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"bitm"))
            .filter_map(|t| t.name.clone())
            .collect();
        let (want, pruned): (Vec<_>, Vec<_>) = want
            .into_iter()
            .filter(|w| w.0 != lightmap_name)
            .partition(|w| present.contains(&w.0));
        let pruned_bitmaps: std::collections::BTreeSet<&str> =
            pruned.iter().map(|w| w.0.as_str()).collect();
        eprintln!(
            "    pinned pruning dropped {} base bitmaps ({} images)",
            pruned_bitmaps.len(),
            pruned.len()
        );
        let converted_lightmap = format!("{}_lightmap", h1convert::convert::scenario_path(name));
        let got: Vec<_> = got
            .into_iter()
            .filter(|g| g.0 != converted_lightmap)
            .filter(|g| !p.textures.bitmaps.iter().any(|(n, _)| *n == g.0))
            .filter(|g| {
                !p.skies.skies.iter().any(|s| {
                    s.bitmaps.iter().any(|b| b.1 == g.0)
                        || s.flares
                            .iter()
                            .flatten()
                            .any(|f| f.bitmap.as_ref().is_some_and(|b| b.0 == g.0))
                })
            })
            // A dome picture's opaque copy (`Sky::opaque_bitmaps`), and a
            // sky flare's bitmap the level added (one of the base's own
            // name is found, not added, and stays the base's image).
            .filter(|g| {
                !p.skies.skies.iter().any(|s| {
                    s.opaque_bitmaps.iter().any(|b| b.1 == g.0)
                        || s.flares.iter().flatten().any(|f| {
                            f.bitmap.as_ref().is_some_and(|b| b.0 == g.0)
                                && !want.iter().any(|w| w.0 == g.0)
                        })
                })
            })
            .collect();
        if got.len() != want.len() {
            let extra: std::collections::BTreeSet<&str> = got
                .iter()
                .filter(|g| !want.iter().any(|w| w.0 == g.0))
                .map(|g| g.0.as_str())
                .collect();
            eprintln!("    extra images' bitmaps: {extra:?}");
        }
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

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_map_has_its_halo1_card() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map("ui").expect("ui.map").clone();
    let ui = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let check = |what: &str, c: &h1convert::ui::Card| {
        assert!(!c.name.is_empty() && !c.description.is_empty(), "{what}");
        assert_eq!((c.width, c.height), h1convert::ui::PICTURE, "{what}");
        assert_eq!(c.rgba.len(), (c.width * c.height * 4) as usize);
        // A picture: lit pixels along its right and bottom edges too.
        let lit = |x: u32, y: u32| {
            let at = ((y * c.width + x) * 4) as usize;
            c.rgba[at..at + 3]
                .iter()
                .map(|&v| u32::from(v))
                .sum::<u32>()
                > 24
        };
        assert!(
            (0..c.height).any(|y| lit(c.width - 1, y)),
            "{what}: right edge"
        );
        assert!(
            (0..c.width).any(|x| lit(x, c.height - 1)),
            "{what}: bottom edge"
        );
        assert!(!c.name.contains('\r') && !c.description.contains('\r'));
        eprintln!("{what}: {} / {}", c.name, c.description);
    };
    for m in h1convert::ui::MP_ORDER {
        let c = h1convert::ui::mp_card(&ui, m).unwrap().unwrap();
        check(m, &c);
    }
    assert_eq!(
        h1convert::ui::mp_card(&ui, "bloodgulch")
            .unwrap()
            .unwrap()
            .name,
        "Blood Gulch"
    );
    for l in h1convert::ui::CAMPAIGN_ORDER {
        let c = h1convert::ui::campaign_card(&ui, l).unwrap().unwrap();
        check(l, &c);
    }
    assert_eq!(
        h1convert::ui::campaign_card(&ui, "a10")
            .unwrap()
            .unwrap()
            .name,
        "The Pillar of Autumn"
    );
}

/// Every image in the map is a texture cache resource (flag `0x200`, as on
/// every retail image; `h1bitmaps::convert::IMAGE_CACHED`). One without it
/// is never loaded and its stage stays empty: the dark lightmap.
fn check_images(name: &str, m: &mut Map<Cursor<Vec<u8>>>) -> Vec<String> {
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    let at = |a: u32, n: usize| {
        let o = a.checked_sub(space.virtual_base)? as usize;
        meta.get(o..o + n)
    };
    let word = |a: u32| at(a, 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
    let mut bad = Vec::new();
    let bitmaps: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm"))
        .filter_map(|t| Some((t.name.clone().unwrap_or_default(), t.location?.address)))
        .collect();
    for (tag, a) in bitmaps {
        let images = word(a + 0x48);
        for k in 0..word(a + 0x44) {
            let flags = at(images + k * 0x74 + 0xe, 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
            if flags.is_some_and(|f| f & h1bitmaps::convert::IMAGE_CACHED == 0) {
                bad.push(format!(
                    "{name}: {tag} image {k}: flags {flags:x?}, not a cache resource"
                ));
            }
        }
    }
    bad
}

/// The Halo 2 sandbox raises each retail vehicle at a Halo 1 spot by how
/// far its model reaches below its origin: Halo 1 holds the origin on the
/// floor, and the retail ghost's hull reaches 0.15 under it.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo2_sandbox_lifts_retail_vehicles_off_the_floor() {
    use h1convert::placements::{lift_vehicles, plan_map, retail_vehicle_depth};
    use h1convert::sandbox::H2Item;
    let mut h2 = H2Disc::open(&std::env::var("H2_DISC").expect("set H2_DISC"));
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(std::env::var("H1_ISO").expect("set H1_ISO")).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map("bloodgulch").unwrap().clone();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let mut base = Map::from_reader(Cursor::new(h2.read(known("bloodgulch").unwrap().1))).unwrap();
    let ctx = tags::object::Context::read(&mut base).unwrap();
    let depth = |r: &str| retail_vehicle_depth(&ctx, r);
    println!("depths: ghost {:?}, warthog {:?}, banshee {:?}", depth("ghost"), depth("warthog"), depth("banshee"));
    // The retail ghost's hull, 0.15 under its origin; the warthog's tyres 2 cm.
    assert!((depth("ghost").unwrap() - 0.1515).abs() < 1e-3);
    assert!((depth("warthog").unwrap() - 0.018).abs() < 1e-3);
    let before = plan_map(&h1).unwrap();
    let mut after = before.clone();
    lift_vehicles(&mut after, depth);
    let mut ghosts = 0;
    for (b, a) in before.items.iter().zip(&after.items) {
        let lift = a.position[2] - b.position[2];
        match b.item {
            H2Item::Vehicle(r) => {
                assert!((lift - depth(r).unwrap().max(0.0)).abs() < 1e-5, "{r}: {lift}");
                ghosts += usize::from(r == "ghost");
            }
            _ => assert_eq!(lift, 0.0),
        }
    }
    assert!(ghosts >= 2, "{ghosts}");
}
