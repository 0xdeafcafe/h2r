//! Every Halo 1 sky on the disc (`$H1_ISO`): what each of its model's
//! shaders draws, and which pictures are drawn opaque because their own
//! alpha is not the shader's (`h1convert::sky_shaders::alpha_reach`).
//! Only names and counts are printed.
//!
//! H1_ISO=disc/halo1.iso cargo test --release -p h1convert --test it sky_pictures_disc:: -- --ignored --nocapture

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::sky::{Sky, opaque_name};
use h1convert::sky_shaders::{Recipe, recipe};
use h1map::H1Map;
use h1map::bitmap::{Bitmap, PixelFormat};

/// The top level's RGBA of image 0 of the bitmap `id`, as stored and as
/// its opaque copy's raw bytes.
fn top(h1: &H1Map, id: h1map::TagId, raw: &[u8]) -> (Vec<u8>, Vec<u8>, PixelFormat) {
    let t = h1.tag(id).unwrap();
    let bm = Bitmap::read(h1, t).unwrap();
    let img = &bm.images[0];
    let stored = h1_assets::bitmaps::decode_image(img, bm.pixels(h1, 0).unwrap()).unwrap();
    let copy = h1_assets::bitmaps::decode_image(img, raw).unwrap();
    (stored[0].rgba.clone(), copy[0].rgba.clone(), img.format)
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_sky_picture_draws_with_the_alpha_halo_1_draws_it_with() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let entries: Vec<_> = disc.maps.clone();
    let (mut skies, mut shaders, mut opaque) = (0, 0, Vec::new());
    for e in &entries {
        let name = h1map::disc::map_stem(e).to_string();
        let h1 = H1Map::from_reader(Cursor::new(iso.read(e).unwrap())).unwrap();
        let tags: Vec<h1map::Tag> = h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"sky "))
            .cloned()
            .collect();
        for tag in tags {
            let sky =
                Sky::from_map(&h1, &tag).unwrap_or_else(|e| panic!("{name} {}: {e}", tag.name));
            skies += 1;
            for s in sky.shaders.iter().flatten() {
                shaders += 1;
                let r = recipe(s);
                println!(
                    "{name} {}: {} chicago {} additive {} maps {:?} alpha {:?} -> {r:?}, opaque picture {:?}",
                    sky.name,
                    s.name,
                    s.chicago,
                    s.additive,
                    s.maps.iter().map(|m| m.scale).collect::<Vec<_>>(),
                    s.maps.iter().map(|m| m.alpha).collect::<Vec<_>>(),
                    s.opaque_picture()
                );
                // Only a chicago dome's picture is drawn without its alpha:
                // the last map, 1×1, after star layers whose alpha it is.
                let Some(k) = s.opaque_picture() else {
                    if let Some(Recipe::Picture(k)) = r {
                        assert!(s.maps[k].alpha, "{name} {}: picture alpha", s.name);
                    }
                    continue;
                };
                assert!(s.chicago && !s.additive, "{name} {}", s.name);
                assert_eq!(
                    k,
                    s.maps.len() - 1,
                    "{name} {}: the picture is last",
                    s.name
                );
                assert_eq!(s.maps[k].scale, [1.0, 1.0], "{name} {}", s.name);
                assert!(s.maps[0].alpha, "{name} {}: the first map's alpha", s.name);
                let id = s.maps[k].bitmap;
                let bitmap_name = h1.tag(id).unwrap().name.clone();
                let (_, n, c) = sky
                    .opaque_bitmaps
                    .iter()
                    .find(|(b, _, _)| *b == id)
                    .unwrap_or_else(|| panic!("{name} {}: no opaque copy", s.name));
                assert_eq!(*n, opaque_name(&bitmap_name));
                let (stored, copy, format) = top(&h1, id, &c.raw[0].0);
                assert!(
                    matches!(format, PixelFormat::X8R8G8B8 | PixelFormat::A8R8G8B8),
                    "{name} {}: {format:?}",
                    s.name
                );
                for (p, q) in stored
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(copy.as_chunks::<4>().0)
                {
                    assert_eq!((&q[..3], q[3]), (&p[..3], 255), "{name} {bitmap_name}");
                }
                let mean = stored
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| u64::from(p[3]))
                    .sum::<u64>()
                    / (stored.len() as u64 / 4);
                println!("    {bitmap_name}: {format:?}, stored alpha mean {mean}, drawn opaque");
                opaque.push((name.clone(), s.name.clone(), format, mean));
            }
        }
    }
    println!(
        "{skies} skies, {shaders} shaders, {} opaque pictures",
        opaque.len()
    );
    // Blood Gulch's clear blue dome: A8R8G8B8 whose alpha (a star mask) is
    // nearly clear; drawn with it, the sky was black.
    let blue = opaque
        .iter()
        .find(|(m, s, _, _)| m == "bloodgulch" && s.ends_with("sky clear blue"))
        .expect("Blood Gulch's dome is drawn opaque");
    assert_eq!(blue.2, PixelFormat::A8R8G8B8);
    assert!(blue.3 < 64, "its stored alpha is mostly clear: {}", blue.3);
    for dome in ["a30", "a50", "b30", "damnation"] {
        assert!(
            opaque.iter().any(|(m, _, _, _)| m == dome),
            "{dome}'s dome is drawn opaque"
        );
    }
}
