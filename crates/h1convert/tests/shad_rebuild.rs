//! The `shad` rebuild (`h1convert::shad`) against both discs. Nothing is
//! written.
//!
//! ```sh
//! H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
//!   cargo test --release -p h1convert --test shad_rebuild -- --ignored --nocapture
//! ```
//!
//! - **Halo 2 side** (`every_layout_matches_its_stems_retail_instances`):
//!   for every retail `shad` of each layout's stem on the Halo 2 disc,
//!   the parameter counts are the layout's, and each constant the layout
//!   writes (default bitmaps, constant vectors) is what the retail
//!   instances hold; each retail bitmap's LOD offset is checked against
//!   `shad::lod_offset` of its size.
//! - **Halo 1 side** (`every_halo1_shader_and_sky_shader_rebuilds`): every
//!   shader of the eight surface types and every sky model's shader on
//!   the ten campaign levels and the multiplayer maps is read, rebuilt
//!   from the definition against Coagulation's stems, read back, and
//!   checked: its template is the layout's stem, every datum in it is the
//!   stem, a default bitmap of the base, or a Halo 1 map's (so nothing of
//!   a Halo 2 shader instance is in it), and its tiling is Halo 1's.
//! - **Texture kinds** (`every_rebuilt_bitmap_is_its_slots_retail_kind`):
//!   each slot of each layout's stem is one kind (2D, cube, volume) on
//!   every retail instance; the base's default bitmaps and every Halo 1
//!   map a rebuilt shader binds are that kind.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::shad::{self, Bm, Bound, Layout, Role, Vector};
use h1map::H1Map;
use map::build::{Block, FixupKind};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Target, Val};
use tags::material::ShaderLibrary;
use tags::shader::SHAD;
use tags::{DecodedTag, TagData, TextureKind};

fn h2_maps(disc: &str) -> Vec<(String, u64, u64)> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc).unwrap())).unwrap();
    let mut out = Vec::new();
    for e in iso.entries().unwrap() {
        let f = e.path.rsplit(['/', '\\']).next().unwrap_or("").to_owned();
        if let Some(s) = f.strip_suffix(".map")
            && !e.is_dir
        {
            out.push((
                s.to_owned(),
                iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
                u64::from(e.size),
            ));
        }
    }
    out
}

fn open(disc: &str, base: u64, size: u64) -> Map<BufReader<File>> {
    Map::from_region(BufReader::new(File::open(disc).unwrap()), base, size).unwrap()
}

#[derive(Default)]
struct Tally {
    instances: usize,
    shape_ok: usize,
    /// (what, agree, total)
    constants: BTreeMap<String, (usize, usize)>,
    lod_ok: usize,
    lod_total: usize,
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_layout_matches_its_stems_retail_instances() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut tallies: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut seen = HashSet::new();
    for (name, base, size) in h2_maps(&disc) {
        let mut map = open(&disc, base, size);
        let Ok(lib) = ShaderLibrary::read(&mut map) else {
            continue;
        };
        let shaders: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group == SHAD && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        let tag_name = |map: &Map<_>, id: TagId| {
            map.tag(id)
                .ok()
                .and_then(|t| t.name.clone())
                .unwrap_or_default()
        };
        for (id, sname) in shaders {
            let Ok(sh) = lib.shader(id) else { continue };
            let stem = tag_name(&map, sh.template.id);
            let Some(layout) = shad::LAYOUTS.iter().find(|l| l.stem == stem) else {
                continue;
            };
            if !seen.insert(sname.clone()) {
                continue;
            }
            let Ok(p) = sh.compiled() else { continue };
            let t = tallies.entry(layout.stem).or_default();
            t.instances += 1;
            if p.bitmaps.len() == layout.bitmaps.len()
                && p.colors.len() == layout.colors.len()
                && p.vectors.len() == layout.vectors.len()
            {
                t.shape_ok += 1;
            } else {
                continue;
            }
            for (k, b) in layout.bitmaps.iter().enumerate() {
                if let Bm::Fixed(f) = b {
                    let e = t.constants.entry(format!("bitmap {k}")).or_default();
                    e.1 += 1;
                    if tag_name(&map, p.bitmaps[k].bitmap) == f.path() {
                        e.0 += 1;
                    }
                }
                if let Bm::Null = b {
                    let e = t.constants.entry(format!("bitmap {k} null")).or_default();
                    e.1 += 1;
                    if p.bitmaps[k].bitmap == TagId::NULL {
                        e.0 += 1;
                    }
                }
            }
            for (k, v) in layout.vectors.iter().enumerate() {
                if let Vector::Const(c) = v {
                    let e = t.constants.entry(format!("vector {k}")).or_default();
                    e.1 += 1;
                    let got = p.vectors[k];
                    if (0..4).all(|i| (got[i] - c[i]).abs() < 1e-4) {
                        e.0 += 1;
                    }
                }
            }
            for b in &p.bitmaps {
                if b.bitmap == TagId::NULL {
                    continue;
                }
                if let Ok(TagData::Bitmap(bm)) = DecodedTag::read(&mut map, b.bitmap).map(|d| d.data)
                    && let Some(i) = bm.images.first()
                {
                    t.lod_total += 1;
                    let want = shad::lod_offset(i.dimensions[0], i.dimensions[1]);
                    if (want - b.lod_offset).abs() < 1e-3 {
                        t.lod_ok += 1;
                    }
                }
            }
        }
        eprintln!("{name}: read");
    }
    let mut bad = Vec::new();
    for l in shad::LAYOUTS {
        let Some(t) = tallies.get(l.stem) else {
            bad.push(format!("{}: no retail instance", l.stem));
            continue;
        };
        eprintln!(
            "{}: {} instances, {} of the layout's shape; LOD offset from size {}/{}",
            l.stem, t.instances, t.shape_ok, t.lod_ok, t.lod_total
        );
        for (what, (agree, total)) in &t.constants {
            eprintln!("    {what}: {agree}/{total}");
            // Every value the layout writes is one retail instances of
            // the stem hold: all of them for a stem constant, some for a
            // per-instance parameter the layout leaves at its neutral
            // value (no reflection cube, no specular mask). A null slot
            // is reported only. The additive sky layer's second stage is
            // the template path's white (with colour 0 half grey, the
            // picture comes back; `sky_shaders`), which no retail
            // instance uses; its tiling rows are those of a uniform map.
            let kept = l.stem == shad::SKY_ADDITIVE.stem
                && matches!(what.as_str(), "bitmap 1" | "vector 2" | "vector 3");
            if *agree == 0 && !what.ends_with("null") && !kept {
                bad.push(format!("{} {what}: {agree}/{total}", l.stem));
            }
        }
        if t.shape_ok != t.instances {
            bad.push(format!(
                "{}: {} of {} instances have the layout's shape",
                l.stem, t.shape_ok, t.instances
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

const CAMPAIGN: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];

/// Every datum fixup in the tree, depth first.
fn datums(b: &Block, out: &mut Vec<TagId>) {
    for f in &b.fixups {
        match &f.kind {
            FixupKind::TagId(id) | FixupKind::TagRef(id) => out.push(*id),
            FixupKind::Block { child, .. } => datums(child, out),
            _ => {}
        }
    }
}

fn floats(r: &defs::Record, path: &str) -> Vec<f32> {
    match r.at(path) {
        Some(Val::List(v)) => v
            .iter()
            .map(|x| match x {
                Val::Float(f) => *f,
                _ => f32::NAN,
            })
            .collect(),
        other => panic!("{path}: {other:?}"),
    }
}

/// Rebuilds one shader with a made-up datum per Halo 1 map and checks
/// it; returns an error text.
fn check_rebuilt(
    lib: &shad::Library,
    layout: &'static Layout,
    maps: &HashMap<Role, Bound>,
    extras: &shad::Extras,
) -> Result<(), String> {
    let tree = lib.build_with(layout, maps, extras).map_err(|e| e.to_string())?;
    let rec = defs::from_built(shad::shad_def(), &tree).map_err(|e| e.to_string())?;
    let stem = lib.stem(layout).unwrap();
    if rec.at("postprocess[0].template") != Some(&Val::Datum(Target::Id(stem))) {
        return Err("postprocess template".into());
    }
    // Every datum: the stem, a default bitmap, or a Halo 1 map's.
    let mut all = Vec::new();
    datums(&tree, &mut all);
    let empty = HashMap::new();
    let defaults = lib.build(layout, &empty).map_err(|e| e.to_string())?;
    let mut allowed = Vec::new();
    datums(&defaults, &mut allowed);
    allowed.extend(maps.values().map(|m| m.id));
    if let Some(x) = all.iter().find(|d| !allowed.contains(d)) {
        return Err(format!("a datum of nothing ours: {x:?}"));
    }
    // Each role the layout draws holds Halo 1's map, at Halo 1's tiling.
    for (k, b) in layout.bitmaps.iter().enumerate() {
        if let Bm::Role(r, _) = b
            && let Some(m) = maps.get(r)
            && rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap"))
                != Some(&Val::Datum(Target::Id(m.id)))
        {
            return Err(format!("bitmap {k} isn't Halo 1's {r:?}"));
        }
    }
    // The colours: the layout's, with the shader's over them.
    for (k, c) in extras.colors_of(layout).iter().enumerate() {
        let got = rec.at(&format!("postprocess[0].colors[{k}].color"));
        if got != Some(&Val::Int(i64::from(*c))) {
            return Err(format!("colour {k} {got:?}, want {c:08x}"));
        }
    }
    for (k, v) in layout.vectors.iter().enumerate() {
        let got = floats(&rec, &format!("postprocess[0].vectors[{k}].vector"));
        let scale = |r: &Role| maps.get(r).map_or([1.0, 1.0], |m| m.scale);
        let (at, want): (usize, Option<[f32; 2]>) = match v {
            Vector::Tile(r) => (0, Some(scale(r))),
            Vector::URow(r) => (0, Some([scale(r)[0], 0.0])),
            Vector::VRow(r) => (0, Some([0.0, scale(r)[1]])),
            Vector::Zw(r) => (2, Some(scale(r))),
            _ => (0, None),
        };
        if let Some(w) = want
            && got[at..at + 2] != w
        {
            return Err(format!("vector {k} {got:?}, want {w:?}"));
        }
    }
    Ok(())
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_halo1_shader_and_sky_shader_rebuilds() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let (_, b, s) = h2_maps(&h2_disc)
        .into_iter()
        .find(|m| m.0 == "coagulation")
        .unwrap();
    let mut base = open(&h2_disc, b, s);
    let lib = shad::Library::read(&mut base);
    for l in shad::LAYOUTS {
        assert!(lib.has(l), "coagulation lacks {}", l.stem);
    }
    let sky = h1convert::sky_shaders::sky_templates(&mut base);

    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names: Vec<&str> = CAMPAIGN
        .iter()
        .copied()
        .chain(h1convert::convert::MAPS.iter().map(|m| m.0))
        .collect();
    // (group, layout or why not) -> count
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut maps_read = 0;
    let mut campaign_read = 0;
    let mut sky_shaders = 0;
    for name in names {
        let Some(e) = disc.map(name).cloned() else {
            eprintln!("{name}: not on the disc");
            continue;
        };
        maps_read += 1;
        campaign_read += usize::from(CAMPAIGN.contains(&name));
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let mut fake = 0u32;
        let mut bound = |scale: [f32; 2], lod: f32| {
            fake += 1;
            Bound {
                id: TagId(0xe500_0000 + fake),
                scale,
                lod_offset: lod,
            }
        };
        for tag in h1.tags() {
            let group = tag.group.to_string();
            if !shad::H1_GROUPS.contains(&group.as_str()) {
                continue;
            }
            let Some(s) = shad::read_h1(&h1, tag) else {
                *counts.entry((group, "no map read".into())).or_default() += 1;
                continue;
            };
            *counts
                .entry((group.clone(), s.layout.stem.rsplit('\\').next().unwrap().into()))
                .or_default() += 1;
            // The first choice, and the fallback a base without its
            // stem draws with.
            for c in s.chain() {
                let maps: HashMap<Role, Bound> = c
                    .maps
                    .iter()
                    .map(|(r, m)| (*r, bound(m.scale, shad::h1_lod_offset(&h1, m.bitmap))))
                    .collect();
                if let Err(e) = check_rebuilt(&lib, c.layout, &maps, &c.extras) {
                    failures.push(format!("{name}: {}: {}: {e}", tag.name, c.layout.stem));
                }
            }
        }
        // The sky models' shaders.
        let refs = h1.multiplayer().map(|m| m.skies).unwrap_or_default();
        for r in refs {
            let Ok(t) = h1.tag(r.id) else { continue };
            let Ok(f) = h1sky::convert::sky_from_map(&h1, t) else {
                failures.push(format!("{name}: sky {} doesn't read", t.name));
                continue;
            };
            for id in f.shaders {
                let Some(s) = h1convert::sky_shaders::read_h1(&h1, id) else {
                    continue;
                };
                let Some(recipe) = h1convert::sky_shaders::recipe(&s) else {
                    continue;
                };
                sky_shaders += 1;
                let maps: Vec<_> = s
                    .maps
                    .iter()
                    .map(|m| {
                        let b = bound(m.scale, m.lod_offset);
                        Some((b.id, m.scale, m.lod_offset))
                    })
                    .collect();
                let layout = sky.layout(recipe).unwrap();
                *counts
                    .entry((
                        "sky".into(),
                        layout.stem.rsplit('\\').next().unwrap().into(),
                    ))
                    .or_default() += 1;
                let Some(tree) = sky.copy(recipe, &maps, recipe.scroll(&s)) else {
                    failures.push(format!("{name}: sky shader {} doesn't build", s.name));
                    continue;
                };
                let rec = defs::from_built(shad::shad_def(), &tree).unwrap();
                let picture = match recipe {
                    h1convert::sky_shaders::Recipe::Picture(k)
                    | h1convert::sky_shaders::Recipe::Additive(k) => k,
                    h1convert::sky_shaders::Recipe::Clouds { tile, .. } => tile,
                };
                let want = maps[picture].unwrap();
                let has = (0..layout.bitmaps.len()).any(|k| {
                    rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap"))
                        == Some(&Val::Datum(Target::Id(want.0)))
                });
                let tiled = (0..layout.vectors.len()).any(|k| {
                    let v = floats(&rec, &format!("postprocess[0].vectors[{k}].vector"));
                    v[0] == want.1[0] || v[1] == want.1[1]
                });
                if !has || !tiled {
                    failures.push(format!("{name}: sky shader {}: picture {has} tiled {tiled}", s.name));
                }
            }
        }
        eprintln!("{name}: read");
    }
    for ((g, l), n) in &counts {
        eprintln!("  {g} -> {l}: {n}");
    }
    eprintln!("{maps_read} maps, {sky_shaders} sky shaders, {} failures", failures.len());
    assert_eq!(campaign_read, CAMPAIGN.len(), "every campaign level");
    assert!(maps_read > campaign_read, "multiplayer maps");
    assert!(failures.is_empty(), "{failures:#?}");
    let _ = FourCC(*b"shad");
}

/// Halo 1's `senv` detail blend for one texel: the base map's alpha
/// blends the primary detail (alpha 0) into the secondary (alpha 1), the
/// blend is multiplied into the base's colour and the micro detail into
/// that, each a biased multiply (`2 × a × b`, saturated; detail function
/// 0, the "double biased multiply" of 394 of 395 retail `senv`).
fn halo1_blend(base: [f32; 4], primary: [f32; 4], secondary: [f32; 4], micro: [f32; 4]) -> [f32; 3] {
    let a = base[3];
    std::array::from_fn(|i| {
        let detail = primary[i] * (1.0 - a) + secondary[i] * a;
        let c = (2.0 * detail * base[i]).clamp(0.0, 1.0);
        (2.0 * c * micro[i]).clamp(0.0, 1.0)
    })
}

struct Texels([[f32; 4]; 4]);

impl shaders::psh::interp::Sampler for Texels {
    fn sample(&self, stage: usize, _: shaders::psh::TextureDim, _: [f32; 3]) -> [f32; 4] {
        self.0[stage]
    }
}

/// `tex_bump_detail_blend_detail`'s colour pass, run on the CPU
/// (`shaders::psh::interp`) for every retail instance on Coagulation
/// with the texels our layout puts in each stage, is Halo 1's blend: the
/// layout's roles are bound where the program reads them, and the final
/// combiner's colours (which the layout writes) are the retail ones the
/// program is run with.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn detail_blend_is_halo1s_rule() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let (_, b, s) = h2_maps(&disc).into_iter().find(|m| m.0 == "coagulation").unwrap();
    let mut map = open(&disc, b, s);
    let mut lib = ShaderLibrary::read(&mut map).unwrap();
    let layout = &shad::TEX_BUMP_DETAIL_BLEND_DETAIL;
    let ids: Vec<(TagId, String)> = map
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut checked = 0;
    let mut texels_run = 0;
    for (id, name) in ids {
        let Ok(sh) = lib.shader(id) else { continue };
        let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
        if stem != layout.stem {
            continue;
        }
        let p = sh.compiled().unwrap().clone();
        assert_eq!(p.colors[..2], layout.colors[..2], "{name}: final combiner colours");
        let m = lib.material(id, None).unwrap();
        let l = m
            .layers
            .iter()
            .find(|l| l.lod == 0 && l.layer == 1)
            .expect("the colour layer");
        let imp = &l.passes[0].implementations[0];
        // The role each stage samples.
        let mut roles = [None; 4];
        for (stage, t) in imp.textures.iter().enumerate() {
            let Some(tags::material::TextureSource::Bitmap { bitmap, .. }) = t.map(|t| t.source) else {
                panic!("{name}: stage {stage} isn't a bitmap");
            };
            let ks: Vec<usize> = (0..p.bitmaps.len()).filter(|&k| p.bitmaps[k].bitmap == bitmap).collect();
            let roles_of: HashSet<Role> = ks
                .iter()
                .filter_map(|&k| match layout.bitmaps[k] {
                    Bm::Role(r, _) => Some(r),
                    _ => None,
                })
                .collect();
            assert_eq!(roles_of.len(), 1, "{name}: stage {stage} reads bitmaps {ks:?}");
            roles[stage] = roles_of.into_iter().next();
        }
        let def = shaders::psh::PixelShaderDef::from_bytes(&imp.pixel_shader).unwrap();
        let (c0, c1) = def.embedded_constants();
        let u = shaders::psh::interp::Uniforms {
            c0,
            c1,
            ..Default::default()
        };
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..2000 {
            let mut texel = || [rnd(), rnd(), rnd(), rnd()];
            let (base, primary, secondary, micro) = (texel(), texel(), texel(), texel());
            let of = |r: Role| match r {
                Role::Base => base,
                Role::Detail => primary,
                Role::Secondary => secondary,
                Role::Micro => micro,
                other => panic!("{name}: {other:?} in the colour pass"),
            };
            let t = Texels(std::array::from_fn(|k| of(roles[k].unwrap())));
            let got = shaders::psh::interp::run(
                &def,
                &shaders::psh::PixelOptions::default(),
                &shaders::psh::interp::Inputs::default(),
                &u,
                &t,
            )
            .unwrap();
            let want = halo1_blend(base, primary, secondary, micro);
            for i in 0..3 {
                assert!(
                    (got[i] - want[i]).abs() < 2e-3,
                    "{name}: base {base:?} primary {primary:?} secondary {secondary:?} micro {micro:?}: {got:?} want {want:?}"
                );
            }
            texels_run += 1;
        }
        checked += 1;
    }
    eprintln!("{checked} retail instances, {texels_run} texels");
    assert!(checked > 0, "coagulation has the stem's instances");
}

/// `tex_bump_env`'s lighting pass, run on the CPU for every retail
/// instance on Coagulation, draws Halo 1's reflection rule: the cube
/// map's colour times a tint and a brightness that are the facing
/// (perpendicular) values where the bump normal faces the eye and the
/// grazing (parallel) ones where it is edge-on. The instance's colours 1
/// to 4 are where the layout says (colours 1 and 2 the stage 2
/// constants, 3 and 4 the stage 3 alphas), so Halo 1's perpendicular
/// colour and brightness go in 1 and 3, the parallel in 2 and 4.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn env_reflection_is_halo1s_rule() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let (_, b, s) = h2_maps(&disc).into_iter().find(|m| m.0 == "coagulation").unwrap();
    let mut map = open(&disc, b, s);
    let mut lib = ShaderLibrary::read(&mut map).unwrap();
    let layout = &shad::TEX_BUMP_ENV;
    let ids: Vec<(TagId, String)> = map
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let argb = |c: u32| {
        let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
        [ch(16), ch(8), ch(0), ch(24)]
    };
    let mut checked = 0;
    for (id, name) in ids {
        let Ok(sh) = lib.shader(id) else { continue };
        let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
        if stem != layout.stem {
            continue;
        }
        let p = sh.compiled().unwrap().clone();
        let m = lib.material(id, None).unwrap();
        let l = m.layers.iter().find(|l| l.lod == 0 && l.layer == 3).expect("the lighting layer");
        let imp = &l.passes[0].implementations[0];
        // Stage 1 the cube map, stage 2 the bump: the layout's roles.
        for (stage, role) in [(1, Role::Cube), (2, Role::Bump)] {
            let Some(tags::material::TextureSource::Bitmap { bitmap, .. }) = imp.textures[stage].map(|t| t.source)
            else {
                panic!("{name}: stage {stage} isn't a bitmap");
            };
            assert!(
                (0..p.bitmaps.len())
                    .any(|k| p.bitmaps[k].bitmap == bitmap && matches!(layout.bitmaps[k], Bm::Role(r, _) if r == role)),
                "{name}: stage {stage} isn't {role:?}"
            );
        }
        let def = shaders::psh::PixelShaderDef::from_bytes(&imp.pixel_shader).unwrap();
        let (mut c0, c1) = def.embedded_constants();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let (c_1, c_2, c_3, c_4) = (argb(p.colors[1]), argb(p.colors[2]), argb(p.colors[3]), argb(p.colors[4]));
        assert!((0..3).all(|i| near(c0[2][i], c_1[i]) && near(c1[2][i], c_2[i])), "{name}: colours 1 and 2");
        assert!(near(c0[3][3], c_3[3]) && near(c1[3][3], c_4[3]), "{name}: colours 3 and 4");
        // Our values: colour 5 (the highlight) black.
        c0[5] = [0.0; 4];
        let run = |c0: &[[f32; 4]; 9], c1: &[[f32; 4]; 9], eye: [f32; 4], cube: [f32; 4]| {
            let u = shaders::psh::interp::Uniforms {
                c0: *c0,
                c1: *c1,
                ..Default::default()
            };
            let t = Texels([[0.5, 0.5, 0.5, 1.0], cube, [0.5, 0.5, 1.0, 1.0], [0.5, 0.5, 0.5, 1.0]]);
            let inputs = shaders::psh::interp::Inputs {
                d0: eye,
                d1: [0.5, 0.5, 1.0, 1.0],
                front: true,
                ..Default::default()
            };
            shaders::psh::interp::run(&def, &shaders::psh::PixelOptions::default(), &inputs, &u, &t).unwrap()
        };
        // The reflection: the colour with the cube map less without it.
        let cube = [0.6, 0.4, 0.2, 0.5];
        let reflection = |tints: [[f32; 3]; 2], bright: [f32; 2], eye: [f32; 4]| {
            let (mut a, mut b) = (c0, c1);
            a[2][..3].copy_from_slice(&tints[0]);
            b[2][..3].copy_from_slice(&tints[1]);
            a[3][3] = bright[0];
            b[3][3] = bright[1];
            let with = run(&a, &b, eye, cube);
            let without = run(&a, &b, eye, [0.0, 0.0, 0.0, cube[3]]);
            [with[0] - without[0], with[1] - without[1], with[2] - without[2]]
        };
        let facing = [0.5, 0.5, 1.0, 1.0];
        let grazing = [1.0, 0.5, 0.5, 1.0];
        let (p1, p2) = ([0.8, 0.4, 0.2], [0.2, 0.4, 0.8]);
        for (eye, own) in [(facing, 0usize), (grazing, 1usize)] {
            let base = {
                let mut t = [p2, p2];
                t[own] = p1;
                let mut b = [0.25, 0.25];
                b[own] = 0.5;
                reflection(t, b, eye)
            };
            assert!(base.iter().all(|c| *c > 1e-3), "{name}: no reflection {base:?}");
            // The other angle's tint and brightness don't show.
            let mut t = [p1, p1];
            t[1 - own] = [1.0, 1.0, 1.0];
            let mut b = [0.5, 0.5];
            b[1 - own] = 1.0;
            let other = reflection(t, b, eye);
            assert!((0..3).all(|i| near(other[i], base[i])), "{name}: {own}: {other:?} vs {base:?}");
            // Proportional to tint × cube per channel, and to brightness.
            let k = base[0] / (p1[0] * cube[0]);
            assert!((0..3).all(|i| (base[i] - k * p1[i] * cube[i]).abs() < 4e-3), "{name}: {base:?} not tint × cube");
            let mut b = [0.25, 0.25];
            b[own] = 0.25;
            let mut t = [p2, p2];
            t[own] = p1;
            let half = reflection(t, b, eye);
            assert!((0..3).all(|i| (half[i] * 2.0 - base[i]).abs() < 4e-3), "{name}: brightness {half:?} vs {base:?}");
        }
        checked += 1;
    }
    eprintln!("{checked} retail instances");
    assert!(checked > 0);
}

/// Every animated retail `two_alpha_clouds` instance on the disc, and
/// every `two_add_clouds` and `plasma_alpha` one animating only one map
/// (`cr_stream`, `needler_ammo_glow`), has [`shad::CLOUDS_TREE`]'s tree, and its ten transform components mean
/// what [`shad::TextureScroll`] writes: 1 and 2 the tile's u and v scale
/// (its rows' values), 0 and 3 one, 6 to 9 zero, the time-driven ones
/// among 4 and 5 (the offsets). Evaluated with the engine's evaluator
/// port (`tags::effects::function`) at time 0.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn cloud_scroll_matches_retail_animated_instances() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let tree = &shad::CLOUDS_TREE;
    let mut seen = HashSet::new();
    let mut checked = 0;
    let mut additive = 0;
    let mut plasma_second = 0;
    for (name, base, size) in h2_maps(&disc) {
        let mut map = open(&disc, base, size);
        let Ok(lib) = ShaderLibrary::read(&mut map) else { continue };
        let ids: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group == SHAD && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, sname) in ids {
            let Ok(sh) = lib.shader(id) else { continue };
            let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
            let plasma = stem == shad::PLASMA_ALPHA.stem;
            let add = stem == shad::TWO_ADD_CLOUDS.stem || plasma;
            if (stem != shad::SKY_CLOUDS.stem && !add) || !seen.insert(sname.clone()) {
                continue;
            }
            let p = sh.compiled().unwrap();
            if p.properties.is_empty() || (add && p.animations.len() != tree.bindings.len()) {
                continue;
            }
            additive += usize::from(add);
            let at = format!("{name}: {sname}");
            assert_eq!(p.lods.len(), 1, "{at}");
            assert_eq!(p.lods[0].layer_mask, 1 << tree.animated_layer, "{at}");
            assert_eq!((p.lods[0].layers.start, u16::from(p.lods[0].layers.count)), (0, tree.layers), "{at}");
            for (k, l) in p.layers.iter().enumerate() {
                let want = match (k as u16).cmp(&tree.animated_layer) {
                    std::cmp::Ordering::Less => (0, 0),
                    std::cmp::Ordering::Equal => (0, 1),
                    std::cmp::Ordering::Greater => (1, 0),
                };
                assert_eq!((l.start, u16::from(l.count)), want, "{at}: layer {k}");
            }
            let a = &p.implementations[0];
            let got = [a.bitmaps, a.render_states, a.stage_states, a.pixel_constants, a.vertex_constants]
                .map(|r| (r.start, u16::from(r.count)));
            assert_eq!(got, tree.implementation, "{at}");
            let binds: Vec<[u8; 4]> = p.animations.iter().map(|b| [b.target, b.b1, b.b2, b.properties]).collect();
            // A plasma_alpha may scroll its first noise instead
            // (`glass_gravunit_glow`): bitmap target 0 and binding words
            // 0x01d2 and 0x01f3, `c[20]` and `c[21]` (register 2 and 3 of
            // bank 1), bitmap 0's rows. The tree the converter writes
            // scrolls the second (`needler_ammo_glow`).
            let first = binds == PLASMA_FIRST_NOISE;
            if !(plasma && first) {
                assert_eq!(binds, tree.bindings, "{at}");
            }
            plasma_second += usize::from(plasma && !first);
            assert_eq!(p.property_refs.len(), 10, "{at}");
            // Component -> value at time 0, and whether time drives it.
            let mut comp = [(f32::NAN, false); 10];
            for r in &p.property_refs {
                let prop = &p.properties[r.property as usize];
                let f = tags::effects::function::Function::parse(&prop.function).unwrap();
                let v = f.evaluate(0.0, 0.0).clamp(0.0, 1.0);
                let v = if f.is_raw_output() { v } else { (f.range[1] - f.range[0]) * v + f.range[0] };
                let timed = f.kind != tags::effects::function::FunctionType::Constant;
                comp[r.component as usize] = (v, timed);
            }
            // The scrolled pair: vectors 0 and 1, plasma_alpha's 2 and 3
            // (0 and 1 when it scrolls its first noise).
            let row = if plasma && !first { 2 } else { 0 };
            assert_eq!(comp[1].0, p.vectors[row][0], "{at}: u scale");
            assert_eq!(comp[2].0, p.vectors[row + 1][1], "{at}: v scale");
            assert!(comp[0] == (1.0, false) && comp[3] == (1.0, false), "{at}: {comp:?}");
            assert!(comp[6..].iter().all(|c| *c == (0.0, false)), "{at}: {comp:?}");
            assert!(comp[4].1 || comp[5].1, "{at}: an offset scrolls");
            assert!(!comp[1].1 && !comp[2].1, "{at}");
            checked += 1;
        }
    }
    eprintln!(
        "{checked} animated cloud layers, {additive} of them two_add_clouds or plasma_alpha, \
         {plasma_second} plasma_alpha scrolling its second noise"
    );
    assert!(checked > additive && additive > 0 && plasma_second > 0);
}

/// A retail `plasma_alpha` scrolling its first noise (bitmap 0) rather
/// than [`shad::CLOUDS_TREE`]'s second.
const PLASMA_FIRST_NOISE: [[u8; 4]; 3] = [[210, 1, 0, 0], [243, 1, 0, 0], [0, 0, 0, 0]];

/// A texture's kind as the slot needs it: Halo 1's "white" placeholder
/// is a 2D texture.
fn h1_kind(h1: &H1Map, id: h1map::TagId) -> Option<TextureKind> {
    let t = h1.tag(id).ok()?;
    let b = h1map::bitmap::Bitmap::read(h1, t).ok()?;
    Some(match b.images.first()?.kind {
        h1map::bitmap::ImageKind::D2 | h1map::bitmap::ImageKind::White => TextureKind::D2,
        h1map::bitmap::ImageKind::D3 => TextureKind::D3,
        h1map::bitmap::ImageKind::Cube => TextureKind::Cube,
        h1map::bitmap::ImageKind::Other(v) => TextureKind::Unknown(v),
    })
}

fn h2_kind<R: std::io::Read + std::io::Seek>(map: &mut Map<R>, id: TagId) -> Option<TextureKind> {
    match DecodedTag::read(map, id).ok()?.data {
        TagData::Bitmap(b) => Some(b.images.first()?.kind),
        _ => None,
    }
}

/// Every texture a rebuilt shader binds is a kind (2D, cube, volume)
/// its stem samples at that slot: a kind retail instances of the stem
/// bind there (Halo 2 disc; one kind per slot, except `water_pool`'s
/// reflection, a cube on 5 and 2D on 9), for the base's default bitmaps
/// the layout names and every Halo 1 map each Halo 1 shader (and each
/// fallback) puts in a slot. A cube in a slot the stem samples as 2D
/// stops the draw (`bumpenvmap needs a D2 texture, bound Cube`).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_rebuilt_bitmap_is_its_slots_retail_kind() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    // Stem -> slot -> kinds seen on retail instances.
    let mut retail: HashMap<&str, BTreeMap<usize, Vec<TextureKind>>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut coag = None;
    for (name, base, size) in h2_maps(&disc) {
        if name == "coagulation" {
            coag = Some((base, size));
        }
        let mut map = open(&disc, base, size);
        let Ok(lib) = ShaderLibrary::read(&mut map) else { continue };
        let ids: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group == SHAD && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, sname) in ids {
            let Ok(sh) = lib.shader(id) else { continue };
            let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
            let Some(layout) = shad::LAYOUTS.iter().find(|l| l.stem == stem) else { continue };
            if !seen.insert(sname) {
                continue;
            }
            let Ok(p) = sh.compiled() else { continue };
            let bitmaps: Vec<TagId> = p.bitmaps.iter().map(|b| b.bitmap).collect();
            for (k, b) in bitmaps.into_iter().enumerate() {
                if b == TagId::NULL || b.0 == u32::MAX {
                    continue;
                }
                if let Some(kind) = h2_kind(&mut map, b) {
                    let v = retail.entry(layout.stem).or_default().entry(k).or_default();
                    if !v.contains(&kind) {
                        v.push(kind);
                    }
                }
            }
        }
    }
    let mut failures = Vec::new();
    let slot_kinds = |stem: &str, k: usize| -> Option<Vec<TextureKind>> {
        retail.get(stem)?.get(&k).cloned()
    };
    for l in shad::LAYOUTS {
        let Some(slots) = retail.get(l.stem) else {
            failures.push(format!("{}: no retail instance", l.stem));
            continue;
        };
        for (k, kinds) in slots {
            let mixed = l.stem == shad::WATER_POOL.stem && *k == 1;
            if kinds.len() != 1 && !mixed {
                failures.push(format!("{} slot {k}: retail kinds {kinds:?}", l.stem));
            }
        }
        eprintln!(
            "{}: {:?}",
            l.stem.rsplit('\\').next().unwrap(),
            (0..l.bitmaps.len()).map(|k| slot_kinds(l.stem, k)).collect::<Vec<_>>()
        );
    }
    // The base's default bitmaps.
    let (b, s) = coag.expect("coagulation");
    let mut base = open(&disc, b, s);
    let find = |base: &Map<_>, path: &str| {
        base.tags()
            .iter()
            .find(|t| t.group == FourCC(*b"bitm") && t.name.as_deref() == Some(path))
            .map(|t| t.id)
    };
    for l in shad::LAYOUTS {
        for (k, bm) in l.bitmaps.iter().enumerate() {
            let f = match bm {
                Bm::Role(_, f) | Bm::Fixed(f) => *f,
                Bm::Null => continue,
            };
            let id = find(&base, f.path()).unwrap_or_else(|| panic!("coagulation lacks {}", f.path()));
            let got = h2_kind(&mut base, id);
            if let Some(want) = slot_kinds(l.stem, k)
                && !got.is_some_and(|g| want.contains(&g))
            {
                failures.push(format!("{} slot {k}: default {} is {got:?}, retail {want:?}", l.stem, f.path()));
            }
        }
    }
    // Every Halo 1 shader's maps, in every link of its chain.
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let h1disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names: Vec<&str> = CAMPAIGN
        .iter()
        .copied()
        .chain(h1convert::convert::MAPS.iter().map(|m| m.0))
        .collect();
    let mut checked = 0;
    for name in names {
        let Some(e) = h1disc.map(name).cloned() else { continue };
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        for tag in h1.tags() {
            if !shad::H1_GROUPS.contains(&tag.group.to_string().as_str()) {
                continue;
            }
            let Some(s) = shad::read_h1(&h1, tag) else { continue };
            for c in s.chain() {
                for (role, m) in &c.maps {
                    let Some(k) = c
                        .layout
                        .bitmaps
                        .iter()
                        .position(|b| matches!(b, Bm::Role(r, _) if r == role))
                    else {
                        failures.push(format!("{name}: {}: {role:?} has no slot", tag.name));
                        continue;
                    };
                    let (Some(want), got) = (slot_kinds(c.layout.stem, k), h1_kind(&h1, m.bitmap)) else {
                        continue;
                    };
                    checked += 1;
                    if !got.is_some_and(|g| want.contains(&g)) {
                        let bname = h1.tag(m.bitmap).map(|t| t.name.clone()).unwrap_or_default();
                        failures.push(format!(
                            "{name}: {} on {}: {role:?} slot {k} wants {want:?}, {bname} is {got:?}",
                            tag.name,
                            c.layout.stem.rsplit('\\').next().unwrap()
                        ));
                    }
                }
            }
        }
    }
    eprintln!("{checked} Halo 1 maps checked, {} failures", failures.len());
    for f in failures.iter().take(60) {
        eprintln!("  {f}");
    }
    assert!(checked > 0);
    assert!(failures.is_empty(), "{} failures", failures.len());
}

/// Every retail `water_pool` instance has [`shad::WATER_TREE`] and the
/// component values its doc gives: the frame (time-driven identity over
/// 0..17), vector 0's x and y in components 1 and 2 (when constant), one
/// in 3, vector 0's z and w in 4 and 5, zeros after; bitmaps 0, 3 and 4
/// 2D. Evaluated at time 0 with `tags::effects::function`.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn water_pool_instances_share_one_tree() {
    use tags::effects::function::{Function, FunctionType};
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let tree = &shad::WATER_TREE;
    let mut seen = HashSet::new();
    let mut checked = 0;
    let mut reflections: BTreeMap<String, usize> = BTreeMap::new();
    for (name, base, size) in h2_maps(&disc) {
        let mut map = open(&disc, base, size);
        let Ok(lib) = ShaderLibrary::read(&mut map) else { continue };
        let ids: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group == SHAD && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, sname) in ids {
            let Ok(sh) = lib.shader(id) else { continue };
            let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
            if !stem.ends_with("\\water_pool") || !seen.insert(sname.clone()) {
                continue;
            }
            let p = sh.compiled().unwrap().clone();
            let at = format!("{name}: {sname}");
            assert_eq!(p.lods.len(), 1, "{at}");
            assert_eq!(p.lods[0].layer_mask, 1 << tree.animated_layer, "{at}");
            assert_eq!(p.layers.len(), usize::from(tree.layers), "{at}");
            let a = &p.implementations[0];
            let got = [a.bitmaps, a.render_states, a.stage_states, a.pixel_constants, a.vertex_constants]
                .map(|r| (r.start, u16::from(r.count)));
            assert_eq!(got, tree.implementation, "{at}");
            let binds: Vec<[u8; 4]> = p.animations.iter().map(|b| [b.target, b.b1, b.b2, b.properties]).collect();
            assert_eq!(binds, tree.bindings, "{at}");
            assert_eq!(p.property_refs.len(), 10, "{at}");
            let mut comp = [(f32::NAN, FunctionType::Constant, [0.0f32; 2]); 10];
            for r in &p.property_refs {
                let prop = &p.properties[r.property as usize];
                let f = Function::parse(&prop.function).unwrap();
                let v = f.evaluate(0.0, 0.0).clamp(0.0, 1.0);
                let v = if f.is_raw_output() { v } else { (f.range[1] - f.range[0]) * v + f.range[0] };
                comp[r.component as usize] = (v, f.kind, f.range);
            }
            let v0 = p.vectors[0];
            assert!(comp[0].1 == FunctionType::Identity && comp[0].2 == [0.0, 17.0], "{at}: frame {:?}", comp[0]);
            for (c, want) in [(1, v0[0]), (2, v0[1]), (3, 1.0), (4, v0[2]), (5, v0[3])] {
                if comp[c].1 == FunctionType::Constant {
                    assert_eq!(comp[c].0, want, "{at}: component {c}");
                }
            }
            assert!(comp[6..].iter().all(|c| c.0 == 0.0 && c.1 == FunctionType::Constant), "{at}");
            for k in [0, 3, 4] {
                assert_eq!(h2_kind(&mut map, p.bitmaps[k].bitmap), Some(TextureKind::D2), "{at}: bitmap {k}");
            }
            let r = format!("{:?}", h2_kind(&mut map, p.bitmaps[1].bitmap));
            *reflections.entry(r).or_default() += 1;
            checked += 1;
        }
    }
    eprintln!("{checked} retail water_pool instances; reflection kinds {reflections:?}");
    assert_eq!(checked, 14);
}

/// Halo 1's additive chicago multiply chains: every one drawn by a model
/// on the disc (D: seven, all fade when parallel) draws on
/// `two_add_clouds`, two maps multiplied, scrolling and faded, the rest
/// folded into the tint. Blood Gulch's teleporter cone in detail: the
/// first dust scrolls on the tile (u 6 s, v −20 s), the green ramp
/// `teleporter_dust` is the still map at 1×1.05, the second dust's mean
/// grey (halved for the stem's doubling) is the tint; opaque facing the
/// eye, gone at a grazing angle. Without the stem: the ramp alone on
/// `one_add_illum`, both dusts' means folded in.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn additive_chicago_chains_draw_two_maps_scrolling_and_faded() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut read = |name: &str| {
        let e = disc.map(name).cloned().unwrap();
        H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
    };
    let chains = [
        (
            "bloodgulch",
            r"scenery\teleporter_base\shaders\teleporter_cone",
        ),
        (
            "a50",
            r"levels\a50\devices\light_ceiling\shaders\volumetric light cone small",
        ),
        (
            "a50",
            r"levels\a50\devices\light_ground\shaders\volumetric light cone ground",
        ),
        ("b40", r"levels\b40\shaders\b40_foglamp"),
        ("a10", r"vehicles\fighterbomber\shaders\afterburners"),
        ("a10", r"vehicles\pelican\shaders\hover thrusters"),
        ("a10", r"vehicles\pelican\shaders\jet thrusters"),
    ];
    let mut maps: HashMap<&str, H1Map> = HashMap::new();
    for (map, shader) in chains {
        let h1 = maps.entry(map).or_insert_with(|| read(map));
        let tag = h1.tags().iter().find(|t| t.name == shader).unwrap();
        let s = shad::read_h1(h1, tag).unwrap();
        assert_eq!(s.layout.stem, shad::TWO_ADD_CLOUDS.stem, "{shader}");
        let t = s.extras.tint.unwrap();
        assert_eq!(
            (t.perpendicular, t.parallel),
            (1.0, 0.0),
            "{shader}: fades when parallel"
        );
        assert!(
            t.color.iter().all(|c| (0.0..=1.0).contains(c)),
            "{shader}: {t:?}"
        );
        let f = s.fallback.as_deref().unwrap();
        assert_eq!(f.layout.stem, shad::ONE_ADD_ILLUM.stem, "{shader}");
        eprintln!(
            "{map}: {shader}: tint {:?}, scroll {:?}",
            t.color, s.extras.scroll
        );
    }

    let h1 = &maps["bloodgulch"];
    let name = |id: h1map::TagId| h1.tag(id).unwrap().name.clone();
    let tag = h1.tags().iter().find(|t| t.name == chains[0].1).unwrap();
    let s = shad::read_h1(h1, tag).unwrap();
    let role = |r: Role| s.maps.iter().find(|(x, _)| *x == r).unwrap().1;
    let dust = r"scenery\teleporter_base\bitmaps\base volumetric dust";
    let ramp = r"scenery\teleporter_base\bitmaps\teleporter_dust";
    assert_eq!(name(role(Role::Tile).bitmap), dust);
    assert_eq!(name(role(Role::Mask).bitmap), ramp);
    assert_eq!(role(Role::Mask).scale, [1.0, 1.05]);
    let [u, v] = s.extras.scroll.unwrap();
    let (u, v) = (u.unwrap(), v.unwrap());
    assert_eq!((u.period, u.from, u.to), (6.0, 0.0, 1.0));
    assert_eq!(
        (v.period, v.from, v.to),
        (20.0, 1.0, 0.0),
        "negative period: backwards"
    );
    let grey = shad::h1_mean_color(h1, role(Role::Tile).bitmap).unwrap();
    let green = shad::h1_mean_color(h1, role(Role::Mask).bitmap).unwrap();
    assert!(
        green[1] > 2.0 * green[0] && green[1] > 2.0 * green[2],
        "the ramp is green: {green:?}"
    );
    assert!(
        (grey[0] - grey[1]).abs() < 0.05 && (grey[1] - grey[2]).abs() < 0.05,
        "{grey:?}"
    );
    let t = s.extras.tint.unwrap();
    for c in 0..3 {
        assert!((t.color[c] - grey[c] * 0.5).abs() < 1e-5, "{t:?} {grey:?}");
    }
    let f = s.fallback.as_deref().unwrap();
    assert_eq!(f.maps.len(), 1);
    assert_eq!(name(f.maps[0].1.bitmap), ramp);
    let t = f.extras.tint.unwrap();
    for c in 0..3 {
        assert!(
            (t.color[c] - grey[c] * grey[c]).abs() < 1e-5,
            "{t:?} {grey:?}"
        );
    }
}

/// Halo 1's plasma `sotr` (`shad::is_plasma`) over the whole disc: the
/// shaders that match are exactly the twelve plasma programs (and b40's
/// structure `b40_shaftenergy`), each on `plasma_alpha`; every other
/// `sotr` keeps its single-map rebuild. Blood Gulch's teleporter shield in
/// detail: `energy plasma` on bitmaps 0 and 1, the second scrolling as
/// Halo 1's map 1 does (u 11 s, v 7 s backwards), the shield picture as
/// the mask, faded when parallel. Every `sotr` is listed by what it
/// becomes (`--nocapture`).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn plasma_sotrs_draw_on_plasma_alpha_and_no_other_sotr_does() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names: Vec<&str> = CAMPAIGN
        .iter()
        .copied()
        .chain(h1convert::convert::MAPS.iter().map(|m| m.0))
        .collect();
    let want: HashSet<&str> = [
        r"characters\jackal\shaders\jackal shield",
        r"scenery\c_field_generator\shaders\generator shield",
        r"powerups\active camoflage\shaders\active camoflage",
        r"vehicles\c_dropship\shaders\dropshield tractor beam",
        r"weapons\needler\shaders\needler core",
        r"powerups\needler ammo\shaders\needler ammo glow",
        r"vehicles\c gun turret\shaders\beam tendril",
        r"levels\c20\devices\index platform\shaders\beam tendril",
        r"powerups\over shield\shaders\over shield",
        r"levels\a50\devices\prison door\shaders\prison energy door",
        r"levels\a50\devices\shuttle bay doors\shaders\shuttle bay doors",
        r"scenery\teleporter_shield\shaders\teleporter_shield",
        r"levels\b40\shaders\b40_shaftenergy",
    ]
    .into_iter()
    .collect();
    let mut got: BTreeMap<String, String> = BTreeMap::new();
    let mut shield = None;
    for name in names {
        let Some(e) = disc.map(name).cloned() else {
            continue;
        };
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        for tag in h1.tags() {
            if tag.group.to_string() != "sotr" || got.contains_key(&tag.name) {
                continue;
            }
            let Some(s) = shad::read_h1(&h1, tag) else {
                got.insert(tag.name.clone(), "no map read".into());
                continue;
            };
            got.insert(
                tag.name.clone(),
                s.layout.stem.rsplit('\\').next().unwrap().into(),
            );
            if tag.name == r"scenery\teleporter_shield\shaders\teleporter_shield"
                && shield.is_none()
            {
                let n = |id: h1map::TagId| h1.tag(id).unwrap().name.clone();
                let maps: Vec<(Role, String, [f32; 2])> = s
                    .maps
                    .iter()
                    .map(|(r, m)| (*r, n(m.bitmap), m.scale))
                    .collect();
                shield = Some((maps, s.extras.clone()));
            }
        }
    }
    for (n, l) in &got {
        eprintln!("  {l:<14} {n}");
    }
    let plasma: HashSet<&str> = got
        .iter()
        .filter(|(_, l)| l.as_str() == "plasma_alpha")
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(plasma, want);

    let (maps, extras) = shield.unwrap();
    let plasma_map = r"levels\a50\devices\prison door\bitmaps\energy plasma".to_owned();
    assert_eq!(
        maps,
        vec![
            (Role::Base, plasma_map.clone(), [1.0, 1.0]),
            (Role::Tile, plasma_map, [1.0, 1.0]),
            (
                Role::Mask,
                r"scenery\teleporter_shield\bitmaps\teleporter_shield".to_owned(),
                [1.0, 1.0]
            ),
        ]
    );
    let [u, v] = extras.scroll.unwrap();
    let (u, v) = (u.unwrap(), v.unwrap());
    assert_eq!((u.period, u.from, u.to), (11.0, 0.0, 1.0));
    assert_eq!((v.period, v.from, v.to), (7.0, 0.0, -1.0));
    let t = extras.tint.unwrap();
    assert_eq!((t.perpendicular, t.parallel), (1.0, 0.0));
    // Colours 0 and 1 the glow, 2 and 3 the lit band, each pair equal
    // (`teleporter_receiver_brightness_against_halo1` checks the values).
    assert_eq!(extras.colors.iter().map(|c| c.0).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(extras.colors[0].1, extras.colors[1].1);
    assert_eq!(extras.colors[2].1, extras.colors[3].1);
    assert!(extras.brightest_mask.is_some_and(|g| (g - 1.329).abs() < 1e-3));
}

/// A Halo 1 bitmap's first image as RGBA8 texels, 0 to 1.
fn h1_texels(h1: &H1Map, id: h1map::TagId) -> Vec<[f32; 4]> {
    use h1bitmaps::BitmapExt;
    let b = h1map::bitmap::Bitmap::read(h1, h1.tag(id).unwrap()).unwrap();
    let img = b.decode(h1, 0, 0).unwrap();
    img.rgba
        .chunks_exact(4)
        .map(|p| [0, 1, 2, 3].map(|k| f32::from(p[k]) / 255.0))
        .collect()
}

/// Per channel: the largest, and the mean, of `f` over `texels`.
fn peak_and_mean(texels: &[[f32; 4]], f: impl Fn(&[f32; 4]) -> [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut peak = [0.0f32; 3];
    let mut sum = [0.0f64; 3];
    for t in texels {
        let v = f(t);
        for k in 0..3 {
            peak[k] = peak[k].max(v[k]);
            sum[k] += f64::from(v[k]);
        }
    }
    (peak, sum.map(|s| (s / texels.len() as f64) as f32))
}

/// Blood Gulch's teleporter receiver, how bright the conversion draws it
/// against Halo 1, face on (both fade alphas 1), per channel.
///
/// The cone: Halo 1 adds `factor × dust × dust × ramp` (the chain's
/// product, `factor` 2 per double multiply); the conversion adds
/// `tile × ramp × 2 × tint` (`two_add_clouds`). The dusts scroll apart, so
/// over time Halo 1's mean is the product of the maps' means and its peak
/// at most the product of their peaks. The conversion's mean must equal
/// Halo 1's; its peak is lower, the dropped dust drawn as its mean.
///
/// The shield: Halo 1's last stage adds `picture.rgb × (band + c0)`, the
/// band 1 but for thin dark lines and `c0` (the glow) stage 6's lower
/// bound, the object having no A output; the conversion
/// (`plasma_alpha`, `shad::PlasmaLight`) adds `mask.a × (colour 2 × q +
/// colour 0 × s)`, the mask the picture with its brightness as alpha.
/// Compared per texel of the picture, lit (Halo 1's band and the stem's
/// `q` 1, `s` 0) and on a dark line (band and `q` 0, `s` at most 1):
/// peaks and means within the tolerances the asserts give.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn teleporter_receiver_brightness_against_halo1() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map("bloodgulch").cloned().unwrap();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let find = |n: &str| h1.tags().iter().find(|t| t.name == n).unwrap();
    let near = |a: [f32; 3], b: [f32; 3], eps: f32| (0..3).all(|k| (a[k] - b[k]).abs() <= eps);
    let below = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| a[k] <= b[k] + 1e-3);

    // The cone.
    let tag = find(r"scenery\teleporter_base\shaders\teleporter_cone");
    let d = h1.tag_data(tag).unwrap();
    let (size, bitmap, _) = h1convert::sky_shaders::SCHI_MAP;
    let elems = h1
        .block(
            h1map::Block::read(d, h1convert::sky_shaders::MAPS).unwrap(),
            size,
        )
        .unwrap();
    let mut factor = 1.0f32;
    let mut h1_peak = [1.0f32; 3];
    let mut h1_mean = [1.0f32; 3];
    let n = elems.len() / size;
    for (k, e) in elems.chunks_exact(size).enumerate() {
        if k + 1 < n {
            let f = u16::from_le_bytes([
                e[shad::CHICAGO_MAP_FUNCTION],
                e[shad::CHICAGO_MAP_FUNCTION + 1],
            ]);
            factor *= if f == shad::CHICAGO_DOUBLE_MULTIPLY {
                2.0
            } else {
                1.0
            };
        }
        let id = h1map::TagRef::read(e, bitmap).unwrap().id;
        let (p, m) = peak_and_mean(&h1_texels(&h1, id), |t| [t[0], t[1], t[2]]);
        for c in 0..3 {
            h1_peak[c] *= p[c];
            h1_mean[c] *= m[c];
        }
    }
    let h1_peak = h1_peak.map(|x| (x * factor).min(1.0));
    let h1_mean = h1_mean.map(|x| (x * factor).min(1.0));
    let s = shad::read_h1(&h1, tag).unwrap();
    assert_eq!(s.layout.stem, shad::TWO_ADD_CLOUDS.stem);
    let role = |r: Role| s.maps.iter().find(|(x, _)| *x == r).unwrap().1.bitmap;
    let tint = s.extras.tint.unwrap().color;
    let (tp, tm) = peak_and_mean(&h1_texels(&h1, role(Role::Tile)), |t| [t[0], t[1], t[2]]);
    let (mp, mm) = peak_and_mean(&h1_texels(&h1, role(Role::Mask)), |t| [t[0], t[1], t[2]]);
    let conv_peak: [f32; 3] = std::array::from_fn(|c| (tp[c] * mp[c] * 2.0 * tint[c]).min(1.0));
    let conv_mean: [f32; 3] = std::array::from_fn(|c| (tm[c] * mm[c] * 2.0 * tint[c]).min(1.0));
    eprintln!(
        "cone: factor {factor}; Halo 1 peak {h1_peak:?} mean {h1_mean:?}; converted peak {conv_peak:?} mean {conv_mean:?}"
    );
    assert!(
        near(conv_mean, h1_mean, 0.01),
        "cone mean {conv_mean:?} vs Halo 1 {h1_mean:?}"
    );
    assert!(
        below(conv_peak, h1_peak),
        "cone peak {conv_peak:?} vs Halo 1 {h1_peak:?}"
    );

    // The shield.
    let tag = find(r"scenery\teleporter_shield\shaders\teleporter_shield");
    let d = h1.tag_data(tag).unwrap();
    let stages = h1
        .block(h1map::Block::read(d, shad::SOTR_STAGES).unwrap(), 0x70)
        .unwrap();
    let last = &stages[6 * 0x70..7 * 0x70];
    let argb = |at: usize| -> [f32; 4] {
        std::array::from_fn(|k| {
            f32::from_le_bytes(last[at + 4 * k..at + 4 * k + 4].try_into().unwrap())
        })
    };
    // The object's A output drives colour 0 (flag bit 2) and the
    // teleporter exports none: the lower bound, (0, 0.329, 0.424).
    assert_ne!(
        u16::from_le_bytes([last[0], last[1]]) & shad::STAGE_A_OUT_COLOR0,
        0
    );
    let lo = argb(0x0c);
    let glow = [lo[1], lo[2], lo[3]];
    assert_eq!(shad::plasma_glow(&h1, d), Some(glow));
    let s = shad::read_h1(&h1, tag).unwrap();
    assert_eq!(s.layout.stem, shad::PLASMA_ALPHA.stem);
    let gain = s.extras.brightest_mask.unwrap();
    let picture_id = s
        .maps
        .iter()
        .find(|(r, _)| *r == Role::Mask)
        .unwrap()
        .1
        .bitmap;
    let picture = h1_texels(&h1, picture_id);
    // The derived mask as the converter writes it (DXT5), decoded.
    let alpha: Vec<f32> = {
        use h1bitmaps::decode_image;
        let b = h1map::bitmap::Bitmap::read(&h1, h1.tag(picture_id).unwrap()).unwrap();
        let (img, px) =
            h1bitmaps::mask::brightest_into_alpha(&b.images[0], b.pixels(&h1, 0).unwrap(), gain)
                .unwrap();
        let levels = decode_image(&img, &px).unwrap();
        let l0 = levels.iter().find(|l| l.face == 0 && l.level == 0).unwrap();
        l0.rgba
            .chunks_exact(4)
            .map(|t| f32::from(t[3]) / 255.0)
            .collect()
    };
    assert_eq!(alpha.len(), picture.len());
    let colour = |k: usize| {
        let c = s.extras.colors.iter().find(|c| c.0 == k).unwrap().1;
        [16, 8, 0].map(|sh| ((c >> sh) & 0xff) as f32 / 255.0)
    };
    let (glow_colour, band_colour) = (colour(0), colour(2));
    assert_eq!((colour(1), colour(3)), (glow_colour, band_colour));
    // Lit (band 1): Halo 1 `min(1, picture × (1 + glow))`; the stem where
    // the noises agree (`s` 0, `q` 1): `mask.a × colour 2`.
    let k: Vec<usize> = (0..picture.len()).collect();
    let (h1_lit_peak, h1_lit_mean) = peak_and_mean(&picture, |t| {
        std::array::from_fn(|c| (t[c] * (1.0 + glow[c])).min(1.0))
    });
    let ours = |f: &dyn Fn(usize) -> [f32; 3]| {
        let rows: Vec<[f32; 4]> = k
            .iter()
            .map(|&i| {
                let v = f(i);
                [v[0], v[1], v[2], 0.0]
            })
            .collect();
        peak_and_mean(&rows, |t| [t[0], t[1], t[2]])
    };
    let (lit_peak, lit_mean) = ours(&|i| band_colour.map(|c| alpha[i] * c));
    // A dark line (Halo 1's band 0): Halo 1 `picture × glow`; the stem
    // there `mask.a × colour 0 × s`, at most `s` 1.
    let (h1_glow_peak, h1_glow_mean) =
        peak_and_mean(&picture, |t| std::array::from_fn(|c| t[c] * glow[c]));
    let (glow_peak, glow_mean) = ours(&|i| glow_colour.map(|c| alpha[i] * c));
    eprintln!(
        "shield: gain {gain}, colours 0/1 {glow_colour:?}, 2/3 {band_colour:?}\n\
         lit:  Halo 1 peak {h1_lit_peak:?} mean {h1_lit_mean:?}; converted peak {lit_peak:?} mean {lit_mean:?}\n\
         glow: Halo 1 peak {h1_glow_peak:?} mean {h1_glow_mean:?}; converted peak {glow_peak:?} mean {glow_mean:?}"
    );
    // The hue's brightest channel (green) peaks where both saturate: the
    // derived alpha is 255 there, a DXT5 endpoint, exact; 0.02 is the
    // 8-bit colours' and the gain's rounding with room.
    assert!(
        (lit_peak[1] - h1_lit_peak[1]).abs() <= 0.02,
        "lit green peak"
    );
    // Means over the picture: DXT5's alpha error is half a step either
    // way and averages out; 0.01 per channel.
    assert!(near(lit_mean, h1_lit_mean, 0.01), "lit mean");
    assert!(near(glow_mean, h1_glow_mean, 0.01), "glow mean");
    // Peaks of the other channels and of the glow: the stem has one hue
    // where Halo 1 colours each texel its own, so a texel greener or
    // bluer than the average differs; within 0.05.
    assert!(near(lit_peak, h1_lit_peak, 0.05), "lit peak");
    assert!(near(glow_peak, h1_glow_peak, 0.05), "glow peak");
}
