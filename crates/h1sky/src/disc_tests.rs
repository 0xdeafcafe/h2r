//! Checks against the owner's discs (`$H1_ISO`, `$H2_ISO`; the Halo 1 and
//! Halo 2 ISOs beside the checkout). Ignored: nothing of the discs is in
//! the repository.
//!
//! - `real_h1_disc_every_sky_reads`: census of every `sky ` tag on all 24
//!   Halo 1 maps.
//! - `real_h2_disc_every_sky_decodes`: every retail Halo 2 sky decodes.
//! - `real_h1_disc_a30_bloodgulch_round_trip`: both skies convert (models
//!   included), build into a v8 map, and read back equal.

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, SeekFrom};

use map::Map;

use crate::convert;
use crate::h1;
use crate::h2;

fn h1_iso() -> std::path::PathBuf {
    std::env::var_os("H1_ISO").map_or_else(
        || concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo1.iso").into(),
        Into::into,
    )
}

fn h2_iso() -> std::path::PathBuf {
    std::env::var_os("H2_ISO").map_or_else(
        || concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into(),
        Into::into,
    )
}

/// Opens one Halo 1 map off the disc.
fn h1_map(x: &mut xiso::Xiso<BufReader<File>>, name: &str) -> Option<h1map::H1Map> {
    let disc = h1map::disc::Disc::open(x).ok()?;
    let entry = disc.map(name)?;
    let base = x.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
    let mut f = BufReader::new(File::open(h1_iso()).ok()?);
    f.seek(SeekFrom::Start(base)).ok()?;
    h1map::H1Map::from_reader(f.take(u64::from(entry.size))).ok()
}

/// `H1_ISO=<iso> cargo test -p h1sky -- --ignored real_h1 --nocapture`
#[test]
#[ignore = "needs the Halo 1 disc ($H1_ISO)"]
fn real_h1_disc_every_sky_reads() {
    let path = h1_iso();
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut x).unwrap();
    assert_eq!(disc.maps.len(), 24, "the retail disc has 24 maps");
    let mut total = 0;
    let mut maps_with_skies = 0;
    let (mut with_model, mut with_animation, mut with_fog, mut with_functions) = (0, 0, 0, 0);
    let mut lights = 0;
    for e in &disc.maps {
        let name = h1map::disc::file_name(e);
        let map = h1_map(&mut x, h1map::disc::map_stem(e)).expect("map reads");
        let skies: Vec<&h1map::Tag> = map
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"sky "))
            .collect();
        if !skies.is_empty() {
            maps_with_skies += 1;
        }
        for t in skies {
            let sky =
                h1::Sky::from_map(&map, t).unwrap_or_else(|e| panic!("{name} {}: {e}", t.name));
            total += 1;
            with_model += usize::from(!sky.model.id.is_none());
            with_animation += usize::from(!sky.animation.id.is_none());
            with_fog += usize::from(!sky.fog.id.is_none());
            with_functions += sky.shader_functions.len();
            lights += sky.lights.len();
            if !sky.model.id.is_none() {
                let m = map.tag(sky.model.id).expect("model resolves");
                assert_eq!(m.group, h1map::Group::new(b"mode"), "{name} model group");
            }
            for l in &sky.lights {
                if !l.lens_flare.id.is_none() {
                    let lens = map.tag(l.lens_flare.id).expect("lens resolves");
                    assert_eq!(lens.group, h1map::Group::new(b"lens"));
                }
                assert_eq!(l.raw.len(), h1::LIGHT_LEN);
            }
            println!(
                "{name}: {} ({} lights, model {}, {:#04x}-byte scalars run)",
                t.name,
                sky.lights.len(),
                if sky.model.id.is_none() { "-" } else { "y" },
                sky.lighting_scalars.len() * 4
            );
        }
    }
    println!(
        "census: {total} skies on {maps_with_skies} of {} maps, {with_model} with a model, \
         {with_animation} with an animation graph, {with_fog} with fog, \
         {with_functions} shader functions, {lights} sky lights",
        disc.maps.len()
    );
    assert_eq!(total, 31, "31 skies on the retail disc");
    assert_eq!(maps_with_skies, 22, "two of the 24 maps have no sky");
    assert_eq!(with_model, 25);
    assert_eq!(with_animation, 2);
    assert_eq!(with_fog, 0);
    assert_eq!(with_functions, 2);
    assert_eq!(lights, 48);
}

/// `H2_ISO=<iso> cargo test -p h1sky -- --ignored real_h2 --nocapture`
#[test]
#[ignore = "needs the Halo 2 disc ($H2_ISO)"]
fn real_h2_disc_every_sky_decodes() {
    let path = h2_iso();
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let entries = x.entries().unwrap();
    let mut maps: Vec<_> = entries
        .iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().starts_with("/maps/"))
        .collect();
    maps.sort_by_key(|e| e.path.clone());
    let mut total = 0;
    let mut lights = 0;
    let mut with_layers = 0;
    let mut with_gradient_b = 0;
    for e in maps {
        let name = e.path.rsplit('/').next().unwrap().to_string();
        let base = x.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        let skies: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"sky "))
            .map(|t| t.id)
            .collect();
        for id in skies {
            let sky = h2::Sky::read_from_map(&mut m, id)
                .unwrap_or_else(|err| panic!("{name} {id}: {err}"));
            total += 1;
            lights += sky.lights.len();
            with_layers += usize::from(!sky.bitmap_layers.is_empty());
            with_gradient_b += usize::from(!sky.gradients_b.is_empty());
            if !sky.model.is_null() {
                let tag = m.tag(sky.model.id).expect("model resolves");
                assert_eq!(tag.group, map::FourCC(*b"mode"), "{name} model group");
            }
        }
        if !name.contains("shared") && !name.contains("mainmenu") {
            println!("{name}: skies decoded so far {total}");
        }
    }
    println!(
        "census: {total} skies, {lights} lights, {with_layers} with bitmap layers, \
         {with_gradient_b} with a second gradient"
    );
    assert_eq!(total, 91, "91 sky identities across the retail maps");
    assert!(lights >= 67, "most skies carry one light");
}

/// The lane's proof: `a30` and `bloodgulch` skies (models included) convert,
/// build into a v8 map, and decode back equal where Halo 2 has the field.
/// `H1_ISO=<iso> cargo test -p h1sky -- --ignored real_h1_round --nocapture`
#[test]
#[ignore = "needs the Halo 1 disc ($H1_ISO)"]
fn real_h1_disc_a30_bloodgulch_round_trip() {
    let path = h1_iso();
    for (map_name, sky_path) in [
        ("a30", "sky\\sky_a30\\skydusk0\\skydusk0"),
        ("bloodgulch", "levels\\test\\bloodgulch\\bloodgulch"),
    ] {
        let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
        let map = h1_map(&mut x, map_name).expect("map reads");
        let tag = map
            .find(h1map::Group::new(b"sky "), sky_path)
            .unwrap_or_else(|| panic!("{map_name} has {sky_path}"));
        let convert::FromMap {
            sky,
            model,
            model_scale,
        } = convert::sky_from_map(&map, tag).expect("sky reads");
        let model = model.unwrap_or_else(|| panic!("{map_name}'s sky has a model"));
        let (bytes, id, model_id) = convert::build_sky_map(
            &sky,
            Some((model.0.as_str(), model.1)),
            &tag.name,
            model_scale,
        )
        .expect("builds");
        let model_id = model_id.expect("the model is in the set");
        let mut built = Map::from_reader(Cursor::new(bytes)).expect("built map reads");
        let out = h2::Sky::read_from_map(&mut built, id).expect("sky decodes");
        assert!(!out.model.is_null());
        assert_eq!(out.model.id, model_id, "the sky points at its model");
        let model_tag = built.tag(model_id).unwrap();
        assert_eq!(model_tag.group, map::FourCC(*b"mode"));
        assert_eq!(out.fallback_rgb, sky.ambient_first_rgb());
        assert_eq!(out.model_scale, model_scale);
        assert_eq!(out.camera_translation_scale, 1.0);
        assert_eq!(out.rotation, 0.0);
        assert_eq!(out.lights.len(), sky.lights.len(), "{map_name} light count");
        for (a, b) in out.lights.iter().zip(&sky.lights) {
            assert_eq!(a.yaw, b.direction[0], "{map_name} yaw");
            assert_eq!(a.pitch, b.direction[1], "{map_name} pitch");
            let p = a.params.as_ref().expect("params");
            assert_eq!(p.color, b.color, "{map_name} colour");
            assert_eq!(p.power, b.power, "{map_name} power");
            assert_eq!(p.falloff_distance, b.falloff_distance, "{map_name} falloff");
            assert_eq!(p.cutoff_angle, b.cutoff_angle, "{map_name} cutoff");
        }
        println!(
            "{map_name}: {} lights, model {} -> datum {model_id}, fallback {:?}",
            sky.lights.len(),
            model.0,
            out.fallback_rgb
        );
    }
}

/// Every Halo 1 sky with a model, converted with its level: no surface of
/// it (but those seen edge-on) comes nearer the camera than the farthest
/// two points of the level are apart, so the level, drawn first, always
/// hides it where they overlap; and skies already that far keep scale 1.
/// a30's distant ground (68 units from the camera on `skydusk1`) is the
/// case that cut through the level at scale 1.
///
/// `H1_ISO=<iso> cargo test -p h1sky -- --ignored real_h1_disc_every_sky_stays --nocapture`
#[test]
#[ignore = "needs the Halo 1 disc ($H1_ISO)"]
fn real_h1_disc_every_sky_stays_behind_its_level() {
    let path = h1_iso();
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut x).unwrap();
    let stems: Vec<String> = disc
        .maps
        .iter()
        .map(|e| h1map::disc::map_stem(e).to_string())
        .collect();
    let (mut checked, mut scaled) = (0, 0);
    for name in stems {
        let map = h1_map(&mut x, &name).expect("map reads");
        let reach = convert::level_reach(&map);
        for t in map
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"sky "))
        {
            let from = convert::sky_from_map(&map, t).expect("sky converts");
            if from.sky.model.id.is_none() {
                continue;
            }
            let m = map.tag(from.sky.model.id).unwrap();
            let (model, _) = h1models::model_from_map(&map, m, &Default::default()).unwrap();
            let nearest = convert::nearest_surface(&model).expect("a sky model has a surface");
            let s = from.model_scale;
            println!(
                "{name} {}: nearest {nearest:.1}, reach {reach:.1}, scale {s:.3}",
                t.name
            );
            assert!(s >= 1.0, "{name} {}: scale {s}", t.name);
            assert!(
                nearest * s >= reach * 0.999,
                "{name} {}: nearest {} inside the level's reach {reach}",
                t.name,
                nearest * s
            );
            if nearest >= reach {
                assert_eq!(s, 1.0, "{name} {}: already behind the level", t.name);
            } else {
                scaled += 1;
            }
            checked += 1;
        }
    }
    println!("{checked} skies checked, {scaled} scaled");
    assert_eq!(checked, 25, "25 skies with a model on the disc");
    assert!(scaled > 0, "a30's sky comes inside its level at scale 1");
}
