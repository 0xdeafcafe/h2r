//! Converted Halo 1 vehicles' cameras: each seat's camera track. Reads
//! `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test vehicle_camera_disc -- --ignored --nocapture
//! ```
//!
//! - `halo_2s_default_track_is_halo_1s`: Halo 2's
//!   `globals\default_unit_camera_track` (what a seat with no track of its
//!   own falls back to) holds Halo 1's `globals\default unit camera track`
//!   point for point, position and orientation: the two games share the
//!   `trak` layout (`tags/src/defs/trak.txt`), and its first point is 3.9
//!   above the camera marker.
//! - `retail_driver_seats_name_a_track`: the retail warthog's and
//!   scorpion's driver seats each name one track (`vehi` seat `+0x74`);
//!   the warthog's highest point is lower than the default's.
//! - `retail_tracks_rise_halo_1s_fall`: the engine finds the camera's
//!   place on a track by bisecting for the look pitch on the elevation of
//!   each point's orientation (`h1objects::rebuild::camera_elevation`),
//!   which rises along every retail track on the base but the default,
//!   and falls along every Halo 1 track on Blood Gulch as stored.
//! - `converted_seats_carry_halo_1s_tracks`: every Blood Gulch vehicle
//!   seat with a Halo 1 camera track names a converted `trak` holding
//!   that track's positions and its orientations conjugated, so the
//!   elevation rises (the warthog's driver its loose track, nine points,
//!   the highest 3.5 up); a seat with none (the scorpion's passengers)
//!   names Halo 1's default unit track, converted the same way, as Halo 1
//!   falls back to it (no Blood Gulch vehicle has a unit track of its own).

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, DEFAULT_UNIT_CAMERA_TRACK, Tree};
use h1objects::rebuild::{SEAT_CAMERA_TRACKS, SEAT_DRIVER, SEAT_FLAGS, VEHI_SEATS, camera_elevation};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H2_DISC", "halo2.iso")).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h1_level(name: &str) -> H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H1_ISO", "halo1.iso")).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

fn reference(r: &Record, path: &str) -> Option<TagId> {
    match r.at(path)? {
        Val::Ref { target: Target::Id(id), .. } | Val::Datum(Target::Id(id)) => Some(*id),
        _ => None,
    }
}

fn text(x: &Record, field: &str, strings: &[Option<String>]) -> String {
    match x.at(field) {
        Some(Val::String(Str::Text(t))) => t.clone(),
        Some(Val::String(Str::Index(w))) => {
            strings.get((w & 0xff_ffff) as usize).cloned().flatten().unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn word(x: &Record, field: &str) -> u32 {
    match x.at(field) {
        Some(Val::Bytes(b)) if b.len() == 4 => u32::from_le_bytes(b[..4].try_into().unwrap()),
        v => panic!("{field}: {v:?}"),
    }
}

fn floats(x: &Record, field: &str) -> Vec<f32> {
    match x.at(field) {
        Some(Val::List(v)) => v
            .iter()
            .map(|f| match f {
                Val::Float(f) => *f,
                x => panic!("{x:?}"),
            })
            .collect(),
        v => panic!("{field}: {v:?}"),
    }
}

/// A `trak` record's control points as (position, orientation).
fn points(t: &Record) -> Vec<(Vec<f32>, Vec<f32>)> {
    let Some(Val::Block(ps)) = t.at("control_points") else { panic!("no control points") };
    ps.iter().map(|p| (floats(p, "position"), floats(p, "orientation"))).collect()
}

/// The track a seat names (its first), if any.
fn seat_track(seat: &Record) -> Option<TagId> {
    match seat.at(SEAT_CAMERA_TRACKS) {
        Some(Val::Block(b)) => b.first().and_then(|x| reference(x, "ref_0")),
        v => panic!("{SEAT_CAMERA_TRACKS}: {v:?}"),
    }
}

fn highest(ps: &[(Vec<f32>, Vec<f32>)]) -> f32 {
    ps.iter().map(|p| p.0[2]).fold(f32::MIN, f32::max)
}

fn same_points(a: &[(Vec<f32>, Vec<f32>)], b: &h1::CameraTrack) -> bool {
    a.len() == b.points.len()
        && a.iter().zip(&b.points).all(|((p, q), (hp, hq))| {
            p.iter().zip(hp).all(|(x, y)| (x - y).abs() < 1e-4)
                && q.iter().zip(hq).all(|(x, y)| (x - y).abs() < 1e-4)
        })
}

/// Each point's elevation (degrees), the key the engine searches by.
fn elevations<'a>(qs: impl IntoIterator<Item = &'a [f32]>) -> Vec<f32> {
    qs.into_iter()
        .map(|q| camera_elevation([q[0], q[1], q[2], q[3]]).to_degrees())
        .collect()
}

fn rises(e: &[f32]) -> bool {
    e.windows(2).all(|w| w[1] > w[0])
}

fn falls(e: &[f32]) -> bool {
    e.windows(2).all(|w| w[1] < w[0])
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo_2s_default_track_is_halo_1s() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let id = base.find(b"trak", "globals\\default_unit_camera_track").unwrap();
    let h2 = points(&base.record(def(b"trak"), id).unwrap());
    let h1 = h1_level("bloodgulch");
    let t = h1
        .tags()
        .iter()
        .find(|t| t.group.0.to_be_bytes() == *b"trak" && t.name == "globals\\default unit camera track")
        .unwrap();
    let ours = h1::CameraTrack::read(&h1, t).unwrap();
    println!("Halo 2 default: {h2:?}");
    println!("Halo 1 default: {:?}", ours.points);
    assert_eq!(ours.points.len(), 9);
    assert!(same_points(&h2, &ours), "Halo 2's default unit track is not Halo 1's");
    assert!(highest(&h2) > 3.8, "the default track's top point is {}", highest(&h2));
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn retail_tracks_rise_halo_1s_fall() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let traks: Vec<(TagId, String)> = base_map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"trak") && base.name(t.id).is_some())
        .filter_map(|t| Some((t.id, t.name.clone()?)))
        .collect();
    let mut retail = 0;
    for (id, name) in &traks {
        let e = elevations(points(&base.record(def(b"trak"), *id).unwrap()).iter().map(|p| &p.1[..]));
        println!("{name}: {e:?}");
        if name == "globals\\default_unit_camera_track" {
            assert!(falls(&e), "{name}");
        } else {
            retail += 1;
            assert!(rises(&e), "{name}");
            assert!(e[0] < -60.0 && *e.last().unwrap() > 60.0, "{name}");
        }
    }
    assert!(retail >= 10, "{retail} retail tracks");
    let h1 = h1_level("bloodgulch");
    let mut n = 0;
    for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"trak") {
        let track = h1::CameraTrack::read(&h1, t).unwrap();
        let e = elevations(track.points.iter().map(|p| &p.1[..]));
        println!("Halo 1 {}: {e:?}", t.name);
        assert!(falls(&e), "{}", t.name);
        n += 1;
    }
    assert!(n >= 3, "{n} Halo 1 tracks");
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_driver_seats_name_a_track() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let strings = base_map.strings().to_vec();
    let default = points(
        &base
            .record(def(b"trak"), base.find(b"trak", "globals\\default_unit_camera_track").unwrap())
            .unwrap(),
    );
    for name in ["objects\\vehicles\\warthog\\warthog", "objects\\vehicles\\scorpion\\scorpion"] {
        let v = base.record(def(b"vehi"), base.find(b"vehi", name).unwrap()).unwrap();
        let Some(Val::Block(seats)) = v.at(VEHI_SEATS) else { panic!() };
        let driver = seats
            .iter()
            .find(|s| word(s, SEAT_FLAGS) & SEAT_DRIVER != 0)
            .unwrap_or_else(|| panic!("{name}: no driver seat"));
        let Some(Val::Block(tracks)) = driver.at(SEAT_CAMERA_TRACKS) else { panic!() };
        assert_eq!(tracks.len(), 1, "{name}: driver tracks");
        let t = points(&base.record(def(b"trak"), seat_track(driver).unwrap()).unwrap());
        println!(
            "{name} {}: {} points, highest {:.2} (default {:.2})",
            text(driver, "label", &strings),
            t.len(),
            highest(&t),
            highest(&default)
        );
        if name.contains("warthog") {
            assert!(highest(&t) < highest(&default));
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn converted_seats_carry_halo_1s_tracks() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let h1 = h1_level("bloodgulch");
    let mut c = Converter::new(&h1, &base);
    let (mut with, mut without) = (0, 0);
    for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
        let unit = h1::Unit::read(&h1, t).unwrap();
        let id = c.object(t).unwrap();
        let v = defs::from_built(def(b"vehi"), &local(&c.tree, id).unwrap().root).unwrap();
        let Some(Val::Block(seats)) = v.at(VEHI_SEATS) else { panic!() };
        assert_eq!(seats.len(), unit.seats.len(), "{}", t.name);
        for (s, h) in seats.iter().zip(&unit.seats) {
            let got = seat_track(s);
            println!("{} {}: Halo 1 tracks {:?}, converted {got:?}", t.name, h.label, h.camera_tracks);
            // A seat with no track takes its unit's, else Halo 1's default.
            let first = match h.camera_tracks.first().or(unit.camera_tracks.first()) {
                Some(&first) => {
                    with += 1;
                    first
                }
                None => {
                    without += 1;
                    h1.tags()
                        .iter()
                        .find(|t| t.group.0.to_be_bytes() == *b"trak" && t.name == DEFAULT_UNIT_CAMERA_TRACK)
                        .expect("Halo 1's default unit track")
                        .id
                }
            };
            let conv = local(&c.tree, got.expect("a converted track")).expect("a local tag");
            assert_eq!(conv.group, FourCC(*b"trak"));
            let rec = defs::from_built(def(b"trak"), &conv.root).unwrap();
            let again = defs::to_built(def(b"trak"), &rec, &mut |_| None).unwrap();
            assert_eq!(defs::from_built(def(b"trak"), &again).unwrap(), rec);
            let mut src = h1::CameraTrack::read(&h1, h1.tag(first).unwrap()).unwrap();
            for p in &mut src.points {
                p.1 = h1models::h1::h2_rotation(p.1);
            }
            let got = points(&rec);
            assert!(same_points(&got, &src), "{} {}", t.name, h.label);
            let e = elevations(got.iter().map(|p| &p.1[..]));
            assert!(rises(&e), "{} {}: {e:?}", t.name, h.label);
            if h.label == "W-driver" {
                assert_eq!(src.points.len(), 9);
                assert!((3.0..4.0).contains(&highest(&got)));
            }
        }
    }
    println!("{with} seats with a track, {without} without");
    assert!(with >= 4 && without >= 4, "{with} with, {without} without");
}
