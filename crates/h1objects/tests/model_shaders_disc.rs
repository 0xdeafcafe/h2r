//! Converted model shaders of a10's cryo tube on the disc: its glass
//! (`sotr` `levels\a10\shaders\cryo glass`) is rebuilt as an alpha
//! blended shader drawing its own glass texture and reflection cube, not
//! the base's stand-in model shader. Reads `$H1_ISO` and `$H2_DISC`;
//! prints names only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test model_shaders_disc -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map};
use h1objects::base::Base;
use h1objects::convert::{Converter, local};
use map::build::{Block, FixupKind};
use map::{Map, TagId};

fn h1_map(name: &str) -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name).expect("map on the disc").clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

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

/// Every tag named in `b` and its child blocks: references (the stem)
/// and datums (the postprocess bitmaps, `Val::Datum`).
fn tag_refs(b: &Block, out: &mut Vec<TagId>) {
    for f in &b.fixups {
        match &f.kind {
            FixupKind::TagRef(id) | FixupKind::TagId(id) => out.push(*id),
            FixupKind::Block { child, .. } | FixupKind::Data(child) | FixupKind::Pointer(child) => {
                tag_refs(child, out)
            }
            _ => {}
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn a10_cryo_tube_glass_is_alpha_blended_with_its_own_maps() {
    let h1 = h1_map("a10");
    // The campaign base a10 converts onto (h1campaign).
    let mut base_map = h2_map("05a_deltaapproach");
    let library = h1convert::shad::Library::read(&mut base_map);
    let blended = library
        .stem(&h1convert::shad::ONE_ALPHA_ENV)
        .expect("the base has one_alpha_env");
    let base = Base::read(&mut base_map).unwrap();
    let stand_in = base
        .model_shader
        .as_ref()
        .expect("the base has a model shader")
        .name
        .clone();
    let mut conv = Converter::new(&h1, &base);
    let tube = h1
        .tags()
        .iter()
        .find(|t| {
            t.group == Group::new(b"mach") && t.name == "levels\\a10\\devices\\cryotube\\cryotube"
        })
        .expect("a10's cryo tube machine");
    conv.object(tube).unwrap();

    let tree = &conv.tree;
    // Only the tree's own (local) IDs name its tags; the stem and default
    // bitmaps are the base's.
    let named = |id: TagId| {
        (id.0 & 0xffff_0000 == local(0).0 & 0xffff_0000)
            .then(|| {
                tree.tags
                    .get((id.0 & 0xffff) as usize)
                    .map(|(n, _)| n.as_str())
            })
            .flatten()
    };
    let (_, glass) = tree
        .tags
        .iter()
        .find(|(n, c)| n == "levels\\a10\\shaders\\cryo glass" && c.group.0 == *b"shad")
        .expect("the cryo glass is a shader of its own, not the stand-in");
    let mut refs = Vec::new();
    tag_refs(&glass.root, &mut refs);
    let names: Vec<&str> = refs.iter().filter_map(|&id| named(id)).collect();
    eprintln!(
        "cryo glass: {} references; tree's: {names:?}; stand-in {stand_in}",
        refs.len()
    );
    assert!(refs.contains(&blended), "drawn with one_alpha_env");
    assert!(
        names.contains(&"levels\\a10\\bitmaps\\cryo glass"),
        "its glass texture"
    );
    assert!(
        names.contains(&"levels\\a10\\bitmaps\\cube map glass"),
        "its reflection cube"
    );
    let report = tree
        .reports
        .iter()
        .find(|r| r.source == tube.name)
        .expect("the tube's report");
    assert!(
        !report.no_home.iter().any(|n| n.contains("cryo glass")),
        "{:?}",
        report.no_home
    );
}

/// The converted base `name` (in `tree`) is DXT5 of the sniper rifle's
/// base map's size and chain, its colours the base map's and its alpha the
/// multipurpose map's red: Halo 1 masks the reflection with that red, and
/// `tex_bump_env` with the colour pass's alpha (`h1bitmaps::mask`). DXT5's
/// alpha is within half a step of its block's 8-value mode (a range of
/// 255 over 14: 19); the colours are re-encoded from Halo 1's DXT1 blocks,
/// exact but where a block used DXT1's three-colour mode.
fn sniper_base_alpha_is_its_multipurpose_red(h1: &H1Map, tree: &h1objects::convert::Tree, name: &str) {
    use h1bitmaps::BitmapExt;
    let (_, conv) = tree.tags.iter().find(|(n, _)| n == name).expect("the masked base");
    assert_eq!(conv.group.0, *b"bitm");
    let images = conv
        .root
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            FixupKind::Block { child, .. } if f.offset == 0x44 => Some(child),
            _ => None,
        })
        .expect("images block");
    let img = h1map::bitmap::Image::parse(&images.bytes).unwrap();
    assert_eq!(img.format, h1map::bitmap::PixelFormat::Dxt5);
    let got = h1bitmaps::decode_image(&img, &conv.raw[0].0).unwrap();
    let read = |path: &str| {
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == path && t.group == Group::new(b"bitm"))
            .unwrap();
        h1map::bitmap::Bitmap::read(h1, t).unwrap()
    };
    let base = read("weapons\\sniper rifle\\bitmaps\\sniper rifle");
    let mask = read("weapons\\sniper rifle\\bitmaps\\sniper rifle multipurpose");
    let b = &base.images[0];
    assert_eq!((img.width, img.height, img.mip_count), (b.width, b.height, b.mip_count));
    let colour = base.decode(h1, 0, 0).unwrap();
    let red = mask.decode(h1, 0, 0).unwrap();
    assert_eq!((red.width, red.height), (colour.width, colour.height), "same size");
    let top = got.iter().find(|l| l.level == 0 && l.face == 0).unwrap();
    let n = (top.width * top.height) as usize;
    let (mut alpha_off, mut colour_off, mut worst) = (0u64, 0u64, 0u8);
    for t in 0..n {
        let d = top.rgba[t * 4 + 3].abs_diff(red.rgba[t * 4]);
        worst = worst.max(d);
        alpha_off += u64::from(d);
        for c in 0..3 {
            colour_off += u64::from(top.rgba[t * 4 + c].abs_diff(colour.rgba[t * 4 + c]));
        }
    }
    let (alpha_mean, colour_mean) = (alpha_off as f64 / n as f64, colour_off as f64 / (3 * n) as f64);
    eprintln!(
        "{name}: alpha off its red by {alpha_mean:.2} on average, {worst} at most; colour by {colour_mean:.2}"
    );
    assert!(worst <= 19, "alpha within half a DXT5 step of the red");
    assert!(alpha_mean < 3.0);
    assert!(colour_mean < 1.0, "the base's colours");
}

/// Blood Gulch's sniper rifle (`weap weapons\sniper rifle\sniper rifle`):
/// its metal `soso` has a reflection cube map, so it is rebuilt on
/// `tex_bump_env` drawing its own base map and that cube, with Halo 1's
/// tints, not the tex_bump copy of its dark base map alone (the first
/// person drew near black); its base map's alpha is its multipurpose
/// map's red, which masks the reflection. A `soso` without a cube map (the assault
/// rifle's display) keeps the tex_bump copy.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_sniper_metal_reflects_its_cube_map() {
    let h1 = h1_map("bloodgulch");
    let mut base_map = h2_map("coagulation");
    let library = h1convert::shad::Library::read(&mut base_map);
    let env = library
        .stem(&h1convert::shad::TEX_BUMP_ENV)
        .expect("the base has tex_bump_env");
    let bump = library
        .stem(&h1convert::shad::TEX_BUMP)
        .expect("the base has tex_bump");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    for name in ["weapons\\sniper rifle\\sniper rifle", "weapons\\assault rifle\\assault rifle"] {
        let w = h1
            .tags()
            .iter()
            .find(|t| t.group == Group::new(b"weap") && t.name == name)
            .expect("the weapon");
        conv.object(w).unwrap();
    }
    let tree = &conv.tree;
    let named = |id: TagId| {
        (id.0 & 0xffff_0000 == local(0).0 & 0xffff_0000)
            .then(|| {
                tree.tags
                    .get((id.0 & 0xffff) as usize)
                    .map(|(n, _)| n.as_str())
            })
            .flatten()
    };
    let shader = |name: &str| {
        let (_, s) = tree
            .tags
            .iter()
            .find(|(n, c)| n == name && c.group.0 == *b"shad")
            .unwrap_or_else(|| panic!("{name}: a shader of the tree"));
        let mut refs = Vec::new();
        tag_refs(&s.root, &mut refs);
        refs
    };
    let metal = shader("weapons\\sniper rifle\\shaders\\sniper rifle metal");
    let names: Vec<&str> = metal.iter().filter_map(|&id| named(id)).collect();
    eprintln!("sniper rifle metal: tree's references {names:?}");
    assert!(metal.contains(&env), "drawn with tex_bump_env");
    // Its base map, with the multipurpose map's red as its alpha
    // (converter version 80).
    let masked = "weapons\\sniper rifle\\bitmaps\\sniper rifle_reflection_sniper rifle multipurpose";
    assert!(names.contains(&masked), "its base map, masked");
    sniper_base_alpha_is_its_multipurpose_red(&h1, tree, masked);
    assert!(
        names.contains(&"vehicles\\warthog\\bitmaps\\reflection diffuse metal"),
        "its reflection cube"
    );
    let display = shader("weapons\\assault rifle\\fp\\shaders\\display");
    assert!(display.contains(&bump), "no cube map: the tex_bump copy");
    assert!(!display.contains(&env));
}

/// Blood Gulch's fern (`scen scenery\plants\plant fern\plant fern`): its
/// `soso` has a DXT3 base three quarters clear and no "not alpha-tested"
/// flag, so it draws on `tex_bump_alpha_test_single_pass` with its own
/// base, cut out as Halo 1 cuts it (the tex_bump copy drew solid cards).
/// The Warthog's hull and the Scorpion's non-reflective hull tile their
/// detail map by their own scale, 16 (`textures::scales_from`), where the
/// copy and the reflection rebuild tiled it once (converter version 84).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_leaves_cut_out_and_details_tile_by_their_scale() {
    use tags::defs::Val;
    let h1 = h1_map("bloodgulch");
    let mut base_map = h2_map("coagulation");
    let library = h1convert::shad::Library::read(&mut base_map);
    let cut = library
        .stem(&h1convert::shad::TEX_BUMP_ALPHA_TEST)
        .expect("the base has tex_bump_alpha_test_single_pass");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    for (group, name) in [
        (b"scen", "scenery\\plants\\plant fern\\plant fern"),
        (b"vehi", "vehicles\\warthog\\warthog"),
        (b"vehi", "vehicles\\scorpion\\scorpion_mp"),
    ] {
        let t = h1
            .tags()
            .iter()
            .find(|t| t.group == Group::new(group) && t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        conv.object(t).unwrap();
    }
    let tree = &conv.tree;
    let shader = |name: &str| {
        &tree
            .tags
            .iter()
            .find(|(n, c)| n == name && c.group.0 == *b"shad")
            .unwrap_or_else(|| panic!("{name}: a shader of the tree"))
            .1
            .root
    };
    let fern = shader("scenery\\plants\\plant fern\\shaders\\plant fern");
    let mut refs = Vec::new();
    tag_refs(fern, &mut refs);
    assert!(refs.contains(&cut), "the fern is alpha-tested");
    let names: Vec<&str> = refs
        .iter()
        .filter_map(|&id| {
            (id.0 & 0xffff_0000 == local(0).0 & 0xffff_0000)
                .then(|| tree.tags.get((id.0 & 0xffff) as usize).map(|(n, _)| n.as_str()))
                .flatten()
        })
        .collect();
    assert!(
        names.contains(&"scenery\\plants\\plant fern\\bitmaps\\plant fern"),
        "its own base: {names:?}"
    );
    // The detail's tiling: the vector the layout binds to the detail.
    let tiling = |name: &str, layout: &h1convert::shad::Layout| {
        let k = layout
            .vectors
            .iter()
            .position(|v| *v == h1convert::shad::Vector::Tile(h1convert::shad::Role::Detail))
            .expect("the layout tiles a detail");
        let rec = tags::defs::from_built(h1convert::shad::shad_def(), shader(name)).unwrap();
        match rec.at(&format!("postprocess[0].vectors[{k}].vector")) {
            Some(Val::List(v)) => v
                .iter()
                .take(2)
                .map(|x| match x {
                    Val::Float(f) => *f,
                    _ => f32::NAN,
                })
                .collect::<Vec<_>>(),
            other => panic!("{name}: {other:?}"),
        }
    };
    let hull = tiling(
        "vehicles\\warthog\\shaders\\warthog hull",
        &h1convert::shad::TEX_BUMP_ENV,
    );
    eprintln!("warthog hull detail tiling {hull:?}");
    assert_eq!(hull, [16.0, 16.0]);
    let plain = tiling(
        "vehicles\\scorpion\\shaders\\hull_non_reflective",
        &h1convert::shad::TEX_BUMP,
    );
    eprintln!("scorpion hull (no reflection) detail tiling {plain:?}");
    assert_eq!(plain, [16.0, 16.0]);
}
