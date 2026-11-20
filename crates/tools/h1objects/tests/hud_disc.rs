//! Every Halo 1 unit HUD (`unhi`) on the Halo 1 disc reads as the elements
//! `h1hud` turns into Halo 2 widgets: each element's bitmap a `bitm` of
//! the same map with a sprite or image in the element's sequence, and its
//! bounds a non-empty part of the image. Reads `$H1_ISO`; prints names and
//! counts only.
//!
//! ```sh
//! H1_ISO=... cargo test -p h1objects --test it hud_disc:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map};
use h1objects::h1hud::{self, Role};

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_unit_hud_reads_with_sprites() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let entries: Vec<_> = h1map::disc::maps(&iso.entries().unwrap());
    let (mut huds, mut maps) = (0, 0);
    for e in entries {
        let name = h1map::disc::map_stem(&e).to_owned();
        let Ok(m) = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())) else {
            continue;
        };
        maps += 1;
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"unhi")) {
            let b = m.tag_data(t).unwrap();
            let els = h1hud::unit_elements(&m, b).unwrap_or_else(|e| panic!("{name} {}: {e}", t.name));
            for el in &els {
                let bt = m.tag(el.bitmap).unwrap();
                assert_eq!(bt.group, Group::new(b"bitm"), "{name} {}", t.name);
                let bm = h1map::bitmap::Bitmap::read(&m, bt).unwrap();
                let (bounds, point) = h1hud::sprite(&bm, el.sequence)
                    .unwrap_or_else(|| panic!("{name} {} {}: nothing in sequence {} of {} ({} sequences)", t.name, el.name, el.sequence, bt.name, bm.sequences.len()));
                let reg = h1hud::registration(el.anchor);
                assert!(bounds[0] < bounds[1] && bounds[2] < bounds[3], "{name} {}", t.name);
                if t.name.contains("cyborg") {
                    eprintln!(
                        "{name} {} {}: {} seq {} anchor {:?} offset {:?} scale {:?} registration {reg:?} (Halo 1's point {point:?})",
                        t.name, el.name, bt.name, el.sequence, el.anchor, el.offset, el.scale
                    );
                }
            }
            if t.name.contains("cyborg") {
                let meters = els.iter().filter(|e| matches!(e.role, Role::Meter(_))).count();
                assert_eq!(meters, 2, "{name} {}: shield and health meters", t.name);
            }
            huds += 1;
        }
    }
    let _ = disc;
    eprintln!("{huds} unit HUDs on {maps} maps");
    assert!(maps >= 10 && huds > 0);
}

/// Prints every weapon HUD (`wphi`) chain of `$H1_MAPS` (default
/// bloodgulch and a10) as `h1hud::weapon_entries` reads it, and checks
/// each drawn entry's bitmap is a `bitm` with something in its sequence.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn weapon_huds_read() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names = std::env::var("H1_MAPS").unwrap_or_else(|_| "bloodgulch,a10".into());
    for name in names.split(',') {
        let e = disc.map(name).expect("map on the disc").clone();
        let m = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        for w in m.tags().iter().filter(|t| t.group == Group::new(b"weap")) {
            let b = m.tag_data(w).unwrap();
            let Some(hud) = h1objects::h1::reference(&m, b, 0x480) else {
                continue;
            };
            let entries = h1hud::weapon_entries(&m, hud)
                .unwrap_or_else(|e| panic!("{name} {}: {e}", w.name));
            eprintln!("{name} {} -> {}", w.name, m.tag(hud).unwrap().name);
            for x in &entries {
                let bt = x.bitmap.map(|b| m.tag(b).unwrap().clone());
                let drawn = match &bt {
                    Some(bt) => {
                        assert_eq!(bt.group, Group::new(b"bitm"));
                        let bm = h1map::bitmap::Bitmap::read(&m, bt).unwrap();
                        h1hud::sprite(&bm, x.sequence).is_some()
                    }
                    None => false,
                };
                eprintln!(
                    "  {} {:?} screen {} hidden {} anchor {:?} offset {:?} scale {:?} colour {:08x} {} seq {} drawn {drawn}",
                    m.tag(x.wphi).unwrap().name,
                    x.kind,
                    x.screen,
                    x.hidden,
                    x.anchor,
                    x.offset,
                    x.scale,
                    x.colour,
                    bt.map_or("-".into(), |t| t.name),
                    x.sequence
                );
            }
        }
    }
}

/// Every map's grenade HUDs (`grhi`, from the globals' grenade types) read
/// as `h1hud::grenade_elements`, each a sprite or image of a `bitm`; prints
/// them, and the colours and scales of every unit and grenade element on
/// the disc.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn grenade_huds_read() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let entries: Vec<_> = h1map::disc::maps(&iso.entries().unwrap());
    let mut looks = std::collections::BTreeMap::<String, usize>::new();
    let mut grenades = 0;
    for e in entries {
        let name = h1map::disc::map_stem(&e).to_owned();
        let Ok(m) = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())) else {
            continue;
        };
        let huds = h1hud::grenade_huds(&m).unwrap();
        for (k, g) in huds.iter().enumerate() {
            let Some(g) = g else { continue };
            let t = m.tag(*g).unwrap();
            assert_eq!(t.group, Group::new(b"grhi"), "{name}");
            let b = m.tag_data(t).unwrap();
            let els = h1hud::grenade_elements(&m, b).unwrap_or_else(|e| panic!("{name} {}: {e}", t.name));
            // Its count: a number element Halo 2 draws as a number text
            // standing where Halo 1's number does (h1hud::number_point).
            if let Some(n) = h1hud::grenade_number(b).unwrap() {
                let w = h1hud::digit_width(&m).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(w, 9, "{name}: ui\\hud\\counter's digit width");
                let h = h1hud::digit_height(&m).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(h, 12, "{name}: hud_counter_numbers' digit height");
                let end = h1hud::number_point(&n, w, h).unwrap_or_else(|e| panic!("{name} {}: {e}", t.name));
                *looks.entry(format!("grhi count {:?} digits {} flags {:#x} ends {end:?}", n.anchor, n.digits, n.flags)).or_default() += 1;
            }
            for el in &els {
                let bt = m.tag(el.bitmap).unwrap();
                let bm = h1map::bitmap::Bitmap::read(&m, bt).unwrap();
                assert!(h1hud::sprite(&bm, el.sequence).is_some(), "{name} {} {}", t.name, el.name);
                if name == "bloodgulch" || name == "a10" {
                    eprintln!(
                        "{name} grenade {k} {} {}: {} seq {} anchor {:?} offset {:?} colour {:08x} scale {:?} (number shown {}, low {})",
                        t.name, el.name, bt.name, el.sequence, el.anchor, el.offset, el.colour, el.scale,
                        b[h1hud::grhi::NUMBER_SHOWN],
                        i16::from_le_bytes([b[h1hud::grhi::LOW], b[h1hud::grhi::LOW + 1]])
                    );
                }
                *looks.entry(format!("grhi {:08x} {:?}", el.colour, el.scale)).or_default() += 1;
            }
            grenades += 1;
        }
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"unhi")) {
            let b = m.tag_data(t).unwrap();
            for el in h1hud::unit_elements(&m, b).unwrap() {
                *looks.entry(format!("unhi {} {:08x} {:?}", el.name, el.colour, el.scale)).or_default() += 1;
            }
        }
    }
    for (k, n) in &looks {
        eprintln!("{n:5} {k}");
    }
    assert!(grenades > 0);
}
