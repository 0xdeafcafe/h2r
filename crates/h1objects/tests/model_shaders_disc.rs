//! Converted model shaders of a10's cryo tube on the disc: its glass
//! (`sotr` `levels\a10\shaders\cryo glass`) is rebuilt as an alpha
//! blended shader drawing its own glass texture and reflection cube. Reads `$H1_ISO` and `$H2_DISC`;
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
        .expect("a rebuilt shader for the cryo glass");
    let mut refs = Vec::new();
    tag_refs(&glass.root, &mut refs);
    let names: Vec<&str> = refs.iter().filter_map(|&id| named(id)).collect();
    eprintln!(
        "cryo glass: {} references; tree's: {names:?}",
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

/// Blood Gulch's player (`bipd characters\cyborg_mp\cyborg_mp`) and
/// Scorpion take change colours as Halo 2's objects do (converter version
/// 84): the cyborg's armour, visor and first-person hands `soso` read
/// change colour C, the Scorpion's darkened hull A, so each is rebuilt on
/// `tex_bump_env_four_change_color` with a change-colour map whose red
/// (slot 0) is the multipurpose map's blue and whose other channels are
/// empty; the biped has four change colours (all empty: Halo 2 writes the
/// player's into them), the Scorpion its Halo 1 grey in slot 0.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_cyborg_and_scorpion_take_change_colours() {
    use tags::defs::{Defs, Val};
    let h1 = h1_map("bloodgulch");
    let mut base_map = h2_map("coagulation");
    let library = h1convert::shad::Library::read(&mut base_map);
    let cc_stem = library
        .stem(&h1convert::shad::TEX_BUMP_ENV_FOUR_CHANGE_COLOR)
        .expect("the base has tex_bump_env_four_change_color");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    for (group, name) in [
        (b"bipd", "characters\\cyborg_mp\\cyborg_mp"),
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
    let named = |id: TagId| {
        (id.0 & 0xffff_0000 == local(0).0 & 0xffff_0000)
            .then(|| {
                tree.tags
                    .get((id.0 & 0xffff) as usize)
                    .map(|(n, _)| n.as_str())
            })
            .flatten()
    };
    let refs = |name: &str| {
        let (_, s) = tree
            .tags
            .iter()
            .find(|(n, c)| n == name && c.group.0 == *b"shad")
            .unwrap_or_else(|| panic!("{name}: a shader of the tree"));
        let mut out = Vec::new();
        tag_refs(&s.root, &mut out);
        out
    };
    for (shader, map) in [
        (
            "characters\\cyborg\\shaders\\armor",
            "characters\\cyborg\\bitmaps\\cyborg multipurpose_change_color_0",
        ),
        (
            "characters\\cyborg\\shaders\\visor",
            "characters\\cyborg\\bitmaps\\cyborg multipurpose_change_color_0",
        ),
        (
            "vehicles\\scorpion\\shaders\\hull_darkened",
            "vehicles\\scorpion\\bitmaps\\scorpion_multipurpose_change_color_0",
        ),
    ] {
        let r = refs(shader);
        let names: Vec<&str> = r.iter().filter_map(|&id| named(id)).collect();
        eprintln!("{shader}: {names:?}");
        assert!(
            r.contains(&cc_stem),
            "{shader}: drawn with the change-colour stem"
        );
        assert!(names.contains(&map), "{shader}: its change-colour map");
    }
    change_colour_map_is_the_multipurpose_blue(
        &h1,
        tree,
        "characters\\cyborg\\bitmaps\\cyborg multipurpose_change_color_0",
        "characters\\cyborg\\bitmaps\\cyborg multipurpose",
    );

    let record = |group: &[u8; 4], name: &str| {
        let (_, c) = tree
            .tags
            .iter()
            .find(|(n, c)| n == name && c.group.0 == *group)
            .unwrap_or_else(|| panic!("{name}"));
        let def = Defs::builtin().group(map::FourCC(*group)).unwrap();
        tags::defs::from_built(def, &c.root).unwrap()
    };
    let count = |r: &tags::defs::Record, p: &str| match r.at(p) {
        Some(Val::Block(b)) => b.len(),
        other => panic!("{p}: {other:?}"),
    };
    let bipd = record(b"bipd", "characters\\cyborg_mp\\cyborg_mp");
    assert_eq!(
        count(&bipd, "unit.obje.change_colors"),
        4,
        "the player's four colours"
    );
    for k in 0..4 {
        assert_eq!(
            count(&bipd, &format!("unit.obje.change_colors[{k}].block_0")),
            0
        );
    }
    let vehi = record(b"vehi", "vehicles\\scorpion\\scorpion_mp");
    assert_eq!(count(&vehi, "unit.obje.change_colors"), 4);
    assert_eq!(
        count(&vehi, "unit.obje.change_colors[0].block_0"),
        1,
        "Halo 1's grey"
    );
    match vehi.at("unit.obje.change_colors[0].block_0[0].unknown_4") {
        Some(Val::Bytes(b)) => {
            let v = f32::from_le_bytes(b[..4].try_into().unwrap());
            assert!((v - 0.45).abs() < 0.01, "lower red {v}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_cyborg_wears_the_chiefs_emblem_on_its_left_shoulder() {
    use h1models::emblem::{LEFT_SHOULDER, place};
    let h1 = h1_map("bloodgulch");
    let mode = h1
        .tags()
        .iter()
        .find(|t| t.group == Group::new(b"mode") && t.name == "characters\\cyborg\\cyborg")
        .expect("the cyborg's model");
    let (model, _) =
        h1models::model_from_map(&h1, mode, &h1models::mode::ModelOptions::default()).unwrap();
    let spot = place(&model, &LEFT_SHOULDER).expect("the ray meets the left upper arm");
    eprintln!("{spot:?}");
    // Halo 1 models face +x, z up: the left shoulder is at +y, near the
    // joint's height (0.569), and its plate faces out to the left.
    assert!((0.1..0.35).contains(&spot.centre[1]), "{:?}", spot.centre);
    assert!((0.4..0.65).contains(&spot.centre[2]), "{:?}", spot.centre);
    assert!(spot.normal[1] > 0.3, "{:?}", spot.normal);
    assert!(
        spot.front_is_geometric,
        "the cyborg's triangles face their normals"
    );

    let mut base_map = h2_map("coagulation");
    let mut base = Base::read(&mut base_map).unwrap();
    let retail_emblem = r"objects\characters\masterchief\shaders\mc_emblem";
    assert!(base.find(b"shad", retail_emblem).is_some());
    base.tags
        .retain(|(_, group, name, _)| *group != map::FourCC(*b"shad") || name != retail_emblem);
    let mut conv = Converter::new(&h1, &base);
    let t = h1
        .tags()
        .iter()
        .find(|t| t.group == Group::new(b"bipd") && t.name == "characters\\cyborg_mp\\cyborg_mp")
        .expect("cyborg_mp");
    conv.object(t).unwrap();
    let (_, m) = conv
        .tree
        .tags
        .iter()
        .find(|(n, c)| n == "characters\\cyborg\\cyborg" && c.group.0 == *b"mode")
        .expect("the converted model");
    let mut refs = Vec::new();
    tag_refs(&m.root, &mut refs);
    let emblem = conv
        .tree
        .tags
        .iter()
        .position(|(name, tag)| {
            name == h1objects::convert::EMBLEM_SHADER && tag.group == map::FourCC(*b"shad")
        })
        .expect("rebuilt emblem material");
    assert!(refs.contains(&local(emblem)));
    let mut bindings = Vec::new();
    tag_refs(&conv.tree.tags[emblem].1.root, &mut bindings);
    for (group, name) in [
        (b"stem", h1convert::shad::EMBLEM_STEM),
        (b"bitm", h1convert::shad::EMBLEM_BITMAP),
    ] {
        assert!(
            bindings.contains(&base.find(group, name).unwrap()),
            "{name}"
        );
    }
}

/// The change-colour map `name` against the Halo 1 multipurpose map
/// `mask`: same size and chain, its red the mask's blue within a DXT
/// step on average, green, blue and alpha 0.
fn change_colour_map_is_the_multipurpose_blue(
    h1: &H1Map,
    tree: &h1objects::convert::Tree,
    name: &str,
    mask: &str,
) {
    use h1bitmaps::BitmapExt;
    let (_, conv) = tree
        .tags
        .iter()
        .find(|(n, _)| n == name)
        .expect("the change-colour map");
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
    let t = h1
        .tags()
        .iter()
        .find(|t| t.name == mask && t.group == Group::new(b"bitm"))
        .unwrap();
    let m = h1map::bitmap::Bitmap::read(h1, t).unwrap();
    let mi = &m.images[0];
    assert_eq!(
        (img.width, img.height, img.mip_count),
        (mi.width, mi.height, mi.mip_count)
    );
    let blue = m.decode(h1, 0, 0).unwrap();
    let top = got.iter().find(|l| l.level == 0 && l.face == 0).unwrap();
    let n = (top.width * top.height) as usize;
    let mut off = 0u64;
    for t in 0..n {
        off += u64::from(top.rgba[t * 4].abs_diff(blue.rgba[t * 4 + 2]));
        for c in 1..4 {
            assert_eq!(top.rgba[t * 4 + c], 0, "texel {t} channel {c}");
        }
    }
    let mean = off as f64 / n as f64;
    eprintln!("{name}: red off the mask's blue by {mean:.2} on average");
    assert!(mean < 4.0);
}

/// Blood Gulch's assault rifle counter (`schi weapons\assault rifle\fp\
/// shaders\numbers`, numeric, limit 60, ten digit images): Halo 1 draws
/// digit `p` of `round(60 × A out)` for the model's shader entry of
/// permutation `p`, and the rifle's A out is its primary ammunition, so
/// the two entries show the rounds loaded, tens (permutation 1) and ones
/// (0). Converted (version 92), each entry is its own `shad` on
/// `illum_detail` drawing the converted digits, its frame picked by the
/// weapon's `primary_ammunition_tens` or `_ones` through retail's counter
/// tree, as the battle rifle's are. Before, both entries drew one
/// `one_add_illum` copy of the digits' first image: "00".
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_assault_rifle_counter_shows_the_rounds_loaded() {
    use h1convert::shad::{AMMUNITION_DIGITS, COUNTER_TREE, ILLUM_DETAIL};
    use tags::defs::{Defs, Str, Target, Val};
    use tags::effects::function::{Function, FunctionType};
    let h1 = h1_map("bloodgulch");
    let mut base_map = h2_map("coagulation");
    let library = h1convert::shad::Library::read(&mut base_map);
    let stem = library
        .stem(&ILLUM_DETAIL)
        .expect("the base has illum_detail");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    let rifle = h1
        .tags()
        .iter()
        .find(|t| {
            t.group == Group::new(b"weap") && t.name == "weapons\\assault rifle\\assault rifle"
        })
        .expect("the assault rifle");
    // Halo 1's side: A out is the primary ammunition over 60 rounds.
    let w = h1objects::h1::Weapon::read(&h1, rifle).unwrap();
    assert_eq!(
        w.rounds_counter(),
        Some(60),
        "A out counts the rounds loaded"
    );
    let fp_tag = h1
        .tag(w.first_person_model.expect("a first-person model"))
        .unwrap()
        .clone();
    let (fp, _) = h1models::model_from_map(&h1, &fp_tag, &Default::default()).unwrap();
    let numbers: Vec<(usize, i16)> = fp
        .shaders
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            h1.tag(s.tag)
                .is_ok_and(|t| t.name == "weapons\\assault rifle\\fp\\shaders\\numbers")
        })
        .map(|(k, s)| (k, s.permutation))
        .collect();
    let mut perms: Vec<i16> = numbers.iter().map(|n| n.1).collect();
    perms.sort();
    assert_eq!(perms, [0, 1], "the counter's two digits: {numbers:?}");
    let n = h1convert::shad::chicago_numeric(&h1, h1.tag(fp.shaders[numbers[0].0].tag).unwrap())
        .expect("a numeric schi");
    assert_eq!((n.limit, n.images, n.engine_number), (60, 10, false));

    conv.object(rifle).unwrap();
    let tree = &conv.tree;
    let index = |name: &str, group: &[u8; 4]| {
        tree.tags
            .iter()
            .position(|(n, c)| n == name && c.group.0 == *group)
            .unwrap_or_else(|| panic!("{name}: a tag of the tree"))
    };
    let record = |k: usize| {
        let c = &tree.tags[k].1;
        let def = Defs::builtin().group(c.group).unwrap();
        tags::defs::from_built(def, &c.root).unwrap()
    };
    assert!(
        !tree
            .tags
            .iter()
            .any(|(n, c)| n == "weapons\\assault rifle\\fp\\shaders\\numbers"
                && c.group.0 == *b"shad"),
        "no single static copy of the digits"
    );
    let digits = local(index(
        "weapons\\assault rifle\\fp\\bitmaps\\numbers_plate",
        b"bitm",
    ));
    let mode = record(index("weapons\\assault rifle\\fp\\fp", b"mode"));
    for &(k, p) in &numbers {
        let input = AMMUNITION_DIGITS[p as usize];
        let place = input.rsplit('_').next().unwrap();
        let name = format!("weapons\\assault rifle\\fp\\shaders\\numbers_{place}");
        let id = local(index(&name, b"shad"));
        // The fp model's material for this entry is that digit.
        match mode.at(&format!("materials[{k}].shader")) {
            Some(Val::Ref {
                target: Target::Id(t),
                ..
            }) => assert_eq!(*t, id, "material {k} (permutation {p}) is {name}"),
            other => panic!("material {k}: {other:?}"),
        }
        let rec = record(index(&name, b"shad"));
        let at = |p: &str| rec.at(p).cloned();
        assert_eq!(
            at("postprocess[0].template"),
            Some(Val::Datum(Target::Id(stem))),
            "{name}"
        );
        assert_eq!(
            at("postprocess[0].bitmaps[0].bitmap"),
            Some(Val::Datum(Target::Id(digits))),
            "{name}"
        );
        assert_eq!(
            at("postprocess[0].properties[0].input"),
            Some(Val::String(Str::Text(input.into()))),
            "{name}"
        );
        let f = match at("postprocess[0].properties[0].function") {
            Some(Val::Data(d)) => Function::parse(&d).unwrap(),
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(
            (f.kind, f.range),
            (FunctionType::Identity, [0.0, 10.0]),
            "{name}"
        );
        assert_eq!(
            at("postprocess[0].lods[0].layer_mask"),
            Some(Val::Int(1 << COUNTER_TREE.animated_layer)),
            "{name}"
        );
        let binds: Vec<i64> = (0..COUNTER_TREE.bindings.len())
            .map(
                |b| match at(&format!("postprocess[0].animations[{b}].target")) {
                    Some(Val::Int(t)) => t,
                    other => panic!("{name}: binding {b}: {other:?}"),
                },
            )
            .collect();
        let want: Vec<i64> = COUNTER_TREE
            .bindings
            .iter()
            .map(|b| i64::from(b[0]))
            .collect();
        assert_eq!(binds, want, "{name}");
        eprintln!("material {k} (permutation {p}): {name}, frame from {input}");
    }
}

/// Blood Gulch's trees and plants: each leaf `soso` (DXT3, three quarters
/// clear) draws on `tex_bump_alpha_test_single_pass` with its leaf bitmap
/// in slot 2, the base the colour pass alpha-tests, and in slot 0, the
/// bump, a flat bump map whose alpha is 0 exactly where the leaf's is
/// below a half and 255 elsewhere (`h1bitmaps::mask::cutout_bump`). The
/// stem's depth and shadow passes kill by slot 0's alpha, as retail's
/// `bush_vine` on coagulation shows: with the default bump there (opaque),
/// the whitebark's leaf canopies drew as whole, near-black cards
/// (converter version 92).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_leaves_cut_their_depth_out_by_the_leaf_alpha() {
    use std::collections::HashSet;
    use tags::defs::{Target, Val};
    use tags::material::{ShaderLibrary, TextureSource};
    let h1 = h1_map("bloodgulch");
    let mut base_map = h2_map("coagulation");

    // Retail: on the cut-out stem, the stages that alpha-kill sample
    // bitmap 0 (bush_vine_bmp) and the alpha-tested passes sample
    // bitmap 2 (bush_vine) at stage 0.
    let vine = base_map
        .tag(map::TagQuery::NameInGroup(
            "scenarios\\objects\\nature\\trees\\bush_vine\\shaders\\bush_vine",
            tags::shader::SHAD,
        ))
        .expect("coagulation's bush_vine")
        .id;
    let mut lib = ShaderLibrary::read(&mut base_map).unwrap();
    let slots: Vec<TagId> = {
        let s = lib.shader(vine).unwrap();
        s.compiled()
            .unwrap()
            .bitmaps
            .iter()
            .map(|b| b.bitmap)
            .collect()
    };
    let m = lib.material(vine, None).unwrap();
    let (mut killed, mut tested) = (HashSet::new(), HashSet::new());
    for i in m
        .layers
        .iter()
        .flat_map(|l| &l.passes)
        .flat_map(|p| &p.implementations)
    {
        let alpha_test = i.render_states.iter().any(|r| {
            tags::shader::render_state_name(r.state) == Some("ALPHATESTENABLE") && r.value != 0
        });
        for (st, t) in i.textures.iter().enumerate() {
            let Some(TextureSource::Bitmap { bitmap, .. }) = t.as_ref().map(|t| t.source) else {
                continue;
            };
            if i.stage_states
                .get(st)
                .is_some_and(|s| s.alpha_kill & 4 != 0)
            {
                killed.insert(bitmap);
            }
            if alpha_test && st == 0 {
                tested.insert(bitmap);
            }
        }
    }
    assert_eq!(
        killed,
        HashSet::from([slots[0]]),
        "the depth passes kill by the bump"
    );
    assert_eq!(
        tested,
        HashSet::from([slots[2]]),
        "the colour pass tests the base"
    );

    let library = h1convert::shad::Library::read(&mut base_map);
    let cut = library
        .stem(&h1convert::shad::TEX_BUMP_ALPHA_TEST)
        .expect("the base has tex_bump_alpha_test_single_pass");
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    for name in [
        "scenery\\trees\\tree_desert_whitebark\\tree_desert_whitebark",
        "scenery\\trees\\tree_desert_dead\\tree_desert_dead",
        "scenery\\shrubs\\shrub_large\\shrub_large",
        "scenery\\plants\\plant fern\\plant fern",
        "scenery\\plants\\plant_broadleaf_short\\plant_broadleaf_short",
    ] {
        let t = h1
            .tags()
            .iter()
            .find(|t| t.group == Group::new(b"scen") && t.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        conv.object(t).unwrap();
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
    let leaves = [
        "scenery\\trees\\tree_desert_whitebark\\shaders\\whitebark_leaves",
        "scenery\\trees\\tree_leafy\\shaders\\tree_leafy_leaves",
        "scenery\\plants\\plant fern\\shaders\\plant fern",
        "scenery\\plants\\plant_broadleaf_tall\\shaders\\broadleaf top",
        "scenery\\plants\\plant_broadleaf_tall\\shaders\\broadleaf bottom",
    ];
    let mut seen = Vec::new();
    for (name, c) in tree.tags.iter().filter(|(_, c)| c.group.0 == *b"shad") {
        let mut refs = Vec::new();
        tag_refs(&c.root, &mut refs);
        if !refs.contains(&cut) {
            assert!(!leaves.contains(&name.as_str()), "{name}: alpha-tested");
            continue;
        }
        let rec = tags::defs::from_built(h1convert::shad::shad_def(), &c.root).unwrap();
        let slot = |k: usize| match rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap")) {
            Some(Val::Datum(Target::Id(id))) => named(*id),
            _ => None,
        };
        let leaf = slot(2).unwrap_or_else(|| panic!("{name}: its own base in slot 2"));
        let bump = slot(0)
            .unwrap_or_else(|| panic!("{name}: a bump of its own in slot 0, not the default"));
        eprintln!("{name}: base {leaf}, bump {bump}");
        assert_eq!(bump, h1convert::shad::cutout_bump_name(leaf), "{name}");
        leaf_alpha_cuts_the_bump(&h1, tree, leaf, bump);
        seen.push(name.as_str());
    }
    for l in leaves {
        assert!(
            seen.contains(&l),
            "{l}: on the cut-out stem with its cut-out bump"
        );
    }
}

/// The converted bump `bump` (in `tree`) is DXT5 of the Halo 1 leaf
/// bitmap `leaf`'s size and chain, its top level's alpha 0 exactly where
/// the leaf's is below 128 and 255 elsewhere, every texel the flat normal.
fn leaf_alpha_cuts_the_bump(h1: &H1Map, tree: &h1objects::convert::Tree, leaf: &str, bump: &str) {
    use h1bitmaps::BitmapExt;
    let (_, conv) = tree.tags.iter().find(|(n, _)| n == bump).expect("the bump");
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
    let t = h1
        .tags()
        .iter()
        .find(|t| t.name == leaf && t.group == Group::new(b"bitm"))
        .unwrap();
    let source = h1map::bitmap::Bitmap::read(h1, t).unwrap();
    let s = &source.images[0];
    assert_eq!(
        (img.width, img.height, img.mip_count),
        (s.width, s.height, s.mip_count)
    );
    let got = h1bitmaps::decode_image(&img, &conv.raw[0].0).unwrap();
    let top = got.iter().find(|l| l.level == 0 && l.face == 0).unwrap();
    let want = source.decode(h1, 0, 0).unwrap();
    assert_eq!((top.width, top.height), (want.width, want.height));
    let n = (top.width * top.height) as usize;
    let mut clear = 0;
    for k in 0..n {
        let a = want.rgba[k * 4 + 3];
        let expect = if a >= h1bitmaps::mask::CUTOUT_THRESHOLD {
            255
        } else {
            0
        };
        assert_eq!(
            top.rgba[k * 4 + 3],
            expect,
            "{bump} texel {k}: leaf alpha {a}"
        );
        let normal = &top.rgba[k * 4..k * 4 + 3];
        assert!(
            normal[0].abs_diff(128) <= 1 && normal[1].abs_diff(128) <= 1 && normal[2] == 255,
            "{normal:?}"
        );
        clear += usize::from(expect == 0);
    }
    eprintln!("{bump}: {clear} of {n} texels cut");
    assert!(
        clear > n / 4 && clear < n,
        "{leaf}: a cut-out, not all or nothing"
    );
}

#[test]
#[ignore = "reads the Halo 1 and Halo 2 discs"]
fn textureless_bsp_draws_use_lightmapped_white_and_destination_tint() {
    use h1convert::{shad, structure, textures};
    use map::build::{TagPayload, TagSet};
    use tags::defs::{self, Defs, Target, Val};
    let mut base = h2_map("05a_deltaapproach");
    for (level, name, glass) in [
        ("d40", r"levels\a10\shaders\burnt flourescent off", false),
        ("a50", r"levels\a50\shaders\covenant glass floor", true),
        ("d20", r"levels\a50\shaders\covenant glass floor", true),
    ] {
        let source = h1_map(level);
        let materials = [(0, name.to_owned())];
        let recipe = textures::plan_bsp(&source, &mut base, &materials, &[]).unwrap();
        let mut set = TagSet::new(map::CacheType::Campaign, level, level);
        let (templates, indices) = recipe
            .install(&mut set, &structure::Templates::default(), &materials)
            .unwrap();
        let read = |id| {
            let tag = set.tag(id).unwrap();
            let TagPayload::Built(root) = &tag.payload else {
                panic!("expected rebuilt tag")
            };
            defs::from_built(Defs::builtin().group(tag.group).unwrap(), root).unwrap()
        };
        let r = read(templates.shader(indices[0]));
        if glass {
            assert_eq!(
                templates.part_kinds[indices[0]],
                structure::PART_KIND_TRANSPARENT
            );
            let Some(Val::Ref {
                target: Target::Id(stem),
                ..
            }) = r.at("template")
            else {
                panic!()
            };
            let t = read(*stem);
            let Some(Val::Ref {
                target: Target::Id(pass),
                ..
            }) = t.at("postprocess[0].passes[0].pass")
            else {
                panic!()
            };
            let pass = read(*pass);
            let Some(Val::Block(states)) = pass.at("postprocess[0].render_states") else {
                panic!()
            };
            for (state, value) in [(59, 1), (62, 0), (63, 0x300), (74, 0x8006), (64, 0)] {
                assert!(
                    states
                        .iter()
                        .any(|s| s.at("state") == Some(&Val::Int(state))
                            && s.at("value") == Some(&Val::Int(value)))
                );
            }
        } else {
            assert_eq!(templates.part_kinds[indices[0]], structure::PART_KIND);
            for (slot, default) in [
                (0, shad::Fixed::DefaultVector),
                (2, shad::Fixed::ColorWhite),
                (3, shad::Fixed::DefaultDetail),
            ] {
                let Some(Val::Datum(Target::Id(b))) =
                    r.at(&format!("postprocess[0].bitmaps[{slot}].bitmap"))
                else {
                    panic!()
                };
                assert_eq!(base.tag(*b).unwrap().name.as_deref(), Some(default.path()));
            }
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 and Halo 2 discs"]
fn b30_fixtures_select_their_own_base_and_reflection_mask() {
    use h1map::bitmap::Bitmap;
    let h1 = h1_map("b30");
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    let mut c = Converter::new(&h1, &base);
    let shader = h1
        .tags()
        .iter()
        .find(|t| t.name == r"levels\b30\devices\light fixtures\shaders\light frame")
        .unwrap();
    let maps = h1convert::textures::h1_maps_of(&h1, shader).unwrap();
    let bitmap = |id| Bitmap::read(&h1, h1.tag(id).unwrap()).unwrap();
    let base_bitmap = bitmap(maps.base.unwrap());
    let mask = bitmap(h1convert::shad::soso_multipurpose(&h1, shader).unwrap());
    let mut selected = Vec::new();
    for (fixture, k) in [("floor1", 1usize), ("ceiling1", 0), ("floor2", 1)] {
        let name = format!(
            r"levels\b30\devices\light fixtures\light fixture {fixture}\light fixture {fixture}"
        );
        let fixture = h1
            .tags()
            .iter()
            .find(|t| t.group == Group::new(b"lifi") && t.name == name)
            .unwrap();
        c.object(fixture).unwrap();
        let index = c
            .tree
            .tags
            .iter()
            .position(|(n, t)| *n == name && t.group.0 == *b"mode")
            .unwrap();
        let id = local(index);
        let mut shaders = Vec::new();
        tag_refs(&c.tree.tags[id.index()].1.root, &mut shaders);
        let name = format!("{}_permutation_{k}", shader.name);
        let index = c
            .tree
            .tags
            .iter()
            .position(|(n, t)| n == &name && t.group.0 == *b"shad")
            .unwrap();
        assert!(shaders.contains(&local(index)));
        let mut bindings = Vec::new();
        tag_refs(&c.tree.tags[index].1.root, &mut bindings);
        let (i, (_, bm)) = c
            .tree
            .tags
            .iter()
            .enumerate()
            .find(|(_, (n, t))| n == &format!("{name}_Base") && t.group.0 == *b"bitm")
            .unwrap();
        assert!(bindings.contains(&local(i)));
        let (_, bytes) = h1bitmaps::mask::red_into_alpha(
            &base_bitmap.images[k],
            base_bitmap.pixels(&h1, k).unwrap(),
            &mask.images[k],
            mask.pixels(&h1, k).unwrap(),
        )
        .unwrap();
        assert_eq!(&bm.raw[0].0[..bytes.len()], bytes.as_slice());
        selected.push(local(i));
    }
    assert_ne!(selected[0], selected[1]);
    assert_eq!(selected[0], selected[2]);
}

#[path = "model_shaders_disc/model_defaults.rs"]
mod model_defaults;

#[path = "model_shaders_disc/model_effects.rs"]
mod model_effects;

#[path = "model_shaders_disc/shield_charger.rs"]
mod shield_charger;
