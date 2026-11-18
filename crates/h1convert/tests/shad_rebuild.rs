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
use tags::{DecodedTag, TagData};

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
                let Some(tree) = sky.copy(recipe, &maps) else {
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
/// blends the secondary detail (alpha 0) into the primary (alpha 1), the
/// blend is multiplied into the base's colour and the micro detail into
/// that, each a biased multiply (`2 × a × b`, saturated; detail function
/// 0, the "double biased multiply" of 394 of 395 retail `senv`).
fn halo1_blend(base: [f32; 4], primary: [f32; 4], secondary: [f32; 4], micro: [f32; 4]) -> [f32; 3] {
    let a = base[3];
    std::array::from_fn(|i| {
        let detail = secondary[i] * (1.0 - a) + primary[i] * a;
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
