//! How bright a converted lightmap draws: Halo 2's structure lighting pass
//! against Halo 1's pages (`h1bitmaps::atlas::to_halo2_light`).
//!
//! Needs `$H2_DISC` and `$H1_ISO`; nothing from either disc is written.

use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;

use assets::{Region, ResourceFiles};
use h1bitmaps::BitmapExt;
use h1map::H1Map;
use map::{FourCC, Map, ResourceFile, TagId};
use tags::material::ShaderLibrary;
use tags::shader::SHAD;
use tags::{DecodedTag, TagData};

fn region(disc: &str, name: &str) -> Region {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap_or_else(|| panic!("no {name}.map on the Halo 2 disc"));
    Region {
        path: PathBuf::from(disc),
        base: iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
        length: e.size.into(),
    }
}

/// `name` from the Halo 2 disc, with the resource files beside it.
fn h2_map(disc: &str, name: &str) -> (Map<BufReader<File>>, ResourceFiles) {
    let r = region(disc, name);
    let map = Map::from_region(BufReader::new(File::open(disc).unwrap()), r.base, r.length).unwrap();
    let mut files = ResourceFiles::new();
    files.set(ResourceFile::Local, r).unwrap();
    for (sel, n) in [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ] {
        files.set(sel, region(disc, n)).unwrap();
    }
    (map, files)
}

fn h1_map(name: &str) -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name).cloned().unwrap();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// The `q` quantile of `v`.
fn quantile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f32 * q) as usize]
}

fn brightest(px: &[u8]) -> f32 {
    f32::from(px[0].max(px[1]).max(px[2])) / 255.0
}

fn mean_rgb(rgba: &[u8]) -> [f32; 3] {
    let mut s = [0f64; 3];
    for px in rgba.chunks(4) {
        for k in 0..3 {
            s[k] += f64::from(px[k]);
        }
    }
    let n = (rgba.len() / 4).max(1) as f64;
    s.map(|x| (x / n / 255.0) as f32)
}

struct Texels([[f32; 4]; 4]);

impl shaders::psh::interp::Sampler for Texels {
    fn sample(&self, stage: usize, _: shaders::psh::TextureDim, _: [f32; 3]) -> [f32; 4] {
        self.0[stage]
    }
}

/// The structure lighting pass (layer 3, the per-pixel implementation)
/// of every retail instance of the opaque stems the converted `senv`
/// draw with, on Coagulation, run on the CPU: with a flat bump (the
/// bump's terms cancel, `t3 × (1 + v1.a × (N·L - L.z))`), whatever the
/// light direction and vertex colours, and no highlight (stage 5's colour
/// black, as the converted shaders have it), the colour is `2 × page ×
/// albedo`, clamped to 1, within three 8-bit steps. Stage 3 is the page;
/// the albedo is stage 0, the screen's copy of the colour pass
/// (`tex_bump_detail_blend_detail`), or stage 0 times stage 1's detail ×
/// 2 (`tex_bump`, which draws its base and detail again here; the detail
/// grey).
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn the_lighting_pass_is_twice_page_times_albedo() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let (mut map, _) = h2_map(&disc, "coagulation");
    let mut lib = ShaderLibrary::read(&mut map).unwrap();
    let stems = [h1convert::shad::TEX_BUMP.stem, h1convert::shad::TEX_BUMP_DETAIL_BLEND_DETAIL.stem];
    let ids: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .map(|t| t.id)
        .collect();
    let mut seed = 0x2468_ace1u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut checked = 0;
    for id in ids {
        let Ok(sh) = lib.shader(id) else { continue };
        let stem = map.tag(sh.template.id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
        if !stems.contains(&stem.as_str()) {
            continue;
        }
        let name = map.tag(id).ok().and_then(|t| t.name.clone()).unwrap_or_default();
        let m = lib.material(id, None).unwrap();
        let l = m.layers.iter().find(|l| l.lod == 0 && l.layer == 3).expect("the lighting layer");
        let imp = &l.passes[0].implementations[0];
        let def = shaders::psh::PixelShaderDef::from_bytes(&imp.pixel_shader).unwrap();
        let (mut c0, mut c1) = def.embedded_constants();
        // Our values: the highlight colour (stage 5's) black, as the
        // layouts write the specular colours.
        c0[5] = [0.0; 4];
        c1[5] = [0.0; 4];
        let u = shaders::psh::interp::Uniforms {
            c0,
            c1,
            ..Default::default()
        };
        for _ in 0..500 {
            let albedo = [rnd(), rnd(), rnd(), rnd()];
            let page = [rnd(), rnd(), rnd(), rnd()];
            // Stage 1: `tex_bump`'s detail map, neutral grey (its stage 0
            // the base: the albedo); the detail blend's cube lookup.
            let t = Texels([albedo, [0.5, 0.5, 0.5, rnd()], [0.5, 0.5, 1.0, 1.0], page]);
            let inputs = shaders::psh::interp::Inputs {
                d0: [rnd(), rnd(), rnd(), rnd()],
                d1: [rnd(), rnd(), rnd(), rnd()],
                front: true,
                ..Default::default()
            };
            let got = shaders::psh::interp::run(&def, &shaders::psh::PixelOptions::default(), &inputs, &u, &t)
                .unwrap();
            for i in 0..3 {
                let want = (2.0 * page[i] * albedo[i]).min(1.0);
                assert!(
                    (got[i] - want).abs() < 0.012,
                    "{name}: page {page:?} albedo {albedo:?}: {got:?}"
                );
            }
        }
        checked += 1;
    }
    eprintln!("{checked} retail instances");
    assert!(checked > 0);
}

/// Blood Gulch lit by Halo 1 and Coagulation, the same valley lit for
/// Halo 2: their ground maps are one albedo (each channel's mean within
/// 0.05), but half of Halo 1's page texels are at full light (1.0 in
/// their brightest channel) where 99 % of Halo 2's are at most 0.85,
/// the pass's ×2 leaving room above. A Halo 1 page is the albedo's
/// multiplier; Halo 2's is half of it.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo1_pages_are_lit_on_twice_halo2s_scale() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let (mut map, mut files) = h2_map(&disc, "coagulation");
    let mut h2 = Vec::new();
    let ltmps: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"ltmp") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    for id in ltmps {
        let lm = tags::structure::Lightmap::read(&mut map, id).unwrap();
        for g in &lm.groups {
            let Some(bid) = g.bitmap else { continue };
            let TagData::Bitmap(b) = DecodedTag::read(&mut map, bid).unwrap().data else {
                panic!("lightmap bitmap");
            };
            let mut done = std::collections::HashSet::new();
            for ri in &g.cluster_render_info {
                if ri.bitmap_index < 0 || ri.palette_index < 0 || !done.insert(ri.bitmap_index) {
                    continue;
                }
                let d = assets::decode_image(&b.images[ri.bitmap_index as usize], &mut files).unwrap();
                let palette = &g.palettes[ri.palette_index as usize];
                let level = d.levels.iter().find(|l| l.face == 0 && l.level == 0).unwrap();
                // P8 decodes as its index; the palette holds the colour.
                for px in level.rgba.chunks(4) {
                    let c = tags::lighting::palette_rgba(palette, px[0]);
                    if c[..3] != [0, 0, 0] {
                        h2.push(brightest(&c));
                    }
                }
            }
        }
    }
    let h1 = h1_map("bloodgulch");
    let mut halo1 = Vec::new();
    for bsp in h1.structure_bsps().unwrap() {
        let s = h1.structure_bsp(&bsp).unwrap();
        let tag = h1.tag(s.lightmaps_bitmap().id).unwrap().clone();
        let pages = h1map::bitmap::Bitmap::read(&h1, &tag).unwrap();
        for i in 0..pages.images.len() {
            let im = pages.decode(&h1, i, 0).unwrap();
            halo1.extend(im.rgba.chunks(4).filter(|px| px[..3] != [0, 0, 0]).map(brightest));
        }
    }
    let (h2_p99, h1_p50) = (quantile(&mut h2, 0.99), quantile(&mut halo1, 0.5));
    eprintln!(
        "Coagulation: {} texels, 99 % at most {h2_p99:.3}; Blood Gulch: {} texels, half at least {h1_p50:.3}",
        h2.len(),
        halo1.len()
    );
    assert!(h2_p99 <= 0.85, "{h2_p99}");
    assert!(h1_p50 >= 0.99, "{h1_p50}");

    // The ground's albedo.
    let ground = map
        .tags()
        .iter()
        .find(|t| t.name.as_deref() == Some("scenarios\\bitmaps\\multi\\halo\\coagulation\\coag_ground"))
        .map(|t| t.id)
        .unwrap();
    let TagData::Bitmap(b) = DecodedTag::read(&mut map, ground).unwrap().data else { panic!() };
    let d = assets::decode_image(&b.images[0], &mut files).unwrap();
    let top = d.levels.iter().filter(|l| l.face == 0).min_by_key(|l| l.level).unwrap();
    let h2_ground = mean_rgb(&top.rgba);
    let t = h1
        .tags()
        .iter()
        .find(|t| t.name == "levels\\test\\bloodgulch\\shaders\\blood ground")
        .unwrap()
        .clone();
    let base = h1convert::textures::h1_maps_of(&h1, &t).unwrap().base.unwrap();
    let bm = h1map::bitmap::Bitmap::read(&h1, h1.tag(base).unwrap()).unwrap();
    let h1_ground = mean_rgb(&bm.decode(&h1, 0, 0).unwrap().rgba);
    eprintln!("ground albedo: Halo 1 {h1_ground:.3?}, Halo 2 {h2_ground:.3?}");
    assert!((0..3).all(|i| (h1_ground[i] - h2_ground[i]).abs() < 0.05));
}
