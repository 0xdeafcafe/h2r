//! Converted Halo 1 vehicles where they meet the ground: their bounding
//! centre (Halo 2 lights an object and finds its cluster from it), their
//! seats' entry tests, and where the Halo 1 sandbox spawns them. Reads
//! `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it vehicle_ground_disc:: -- --ignored --nocapture
//! ```
//!
//! - `bounding_centres_sit_inside_the_body`: the retail warthog and its
//!   turrets hold their bounding centres 0.29 to 0.5 above their origins,
//!   inside the body (the ghost, banshee, scorpion and wraith keep theirs
//!   on the origin, printed); every converted vehicle of every
//!   multiplayer map holds it at least 0.1 above its lowest point; on Blood Gulch each spawn's centre is
//!   in open space with the floor under it within the lighting ray's 10
//!   units.
//!   Halo 1's own sphere, on the origin, was 1 mm above the floor there.
//! - `every_seat_admits_a_rider_at_its_entry`: `0x000c9040`'s entry tests
//!   run for a rider standing round each seat's entry marker. The retail
//!   warthog's and ghost's driver and passenger seats admit one (the
//!   control); every converted Blood Gulch seat does; with the facing
//!   angle the warthog role held, the converted warthog's driver seat
//!   refuses a rider square beside its door and admits one only from
//!   ahead of it, looking back.
//! - `halo1_sandbox_spawns_vehicles_above_the_floor`: Halo 1 places
//!   every Blood Gulch vehicle's origin on the floor, so the ghost's hull
//!   started 0.14 into it; the sandbox's spawns now hold every vehicle's
//!   lowest point on the floor.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::rebuild::{ANY_FACING, SEAT_BOARDING, SEAT_ENTRY_FACING, SEAT_FLAGS, VEHI_SEATS};
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
    h1_level_on_disc(name).unwrap()
}

/// The level, or `None` where the Xbox disc doesn't carry it.
fn h1_level_on_disc(name: &str) -> Option<H1Map> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H1_ISO", "halo1.iso")).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
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

fn float(x: &Record, field: &str) -> f32 {
    match x.at(field) {
        Some(Val::Float(f)) => *f,
        Some(Val::Bytes(b)) if b.len() == 4 => f32::from_le_bytes(b[..4].try_into().unwrap()),
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

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `v` rotated by the unit quaternion `q` (`i, j, k, w`).
fn rotate(q: &[f32], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// A marker in object space at the rest pose: its position and forward
/// (first axis). The node's model-space rotation is the transpose of its
/// inverse basis (`mode` node `+0x2c..+0x50`, docs/halo1/models.md).
#[derive(Clone, Copy, Debug)]
struct Frame {
    at: [f32; 3],
    forward: [f32; 3],
}

/// Every marker of `mode` by name (first instance).
fn frames(mode: &Record, strings: &[Option<String>]) -> BTreeMap<String, Frame> {
    let Some(Val::Block(nodes)) = mode.at("nodes") else { panic!() };
    let pose: Vec<([Vec<f32>; 3], [f32; 3])> = nodes
        .iter()
        .map(|n| {
            let c = [floats(n, "inverse_forward"), floats(n, "inverse_left"), floats(n, "inverse_up")];
            let ip = floats(n, "inverse_position");
            let p = [-dot(&c[0], &ip), -dot(&c[1], &ip), -dot(&c[2], &ip)];
            (c, p)
        })
        .collect();
    let mut out = BTreeMap::new();
    let Some(Val::Block(groups)) = mode.at("marker_groups") else { panic!() };
    for g in groups {
        let name = text(g, "name", strings);
        let Some(Val::Block(ms)) = g.at("markers") else { continue };
        let Some(m) = ms.first() else { continue };
        let node = match m.at("node") {
            Some(Val::Int(i)) => *i as usize,
            v => panic!("{v:?}"),
        };
        let (c, p) = &pose[node];
        let obj = |v: [f32; 3]| [dot(&c[0], &v), dot(&c[1], &v), dot(&c[2], &v)];
        let t = obj(floats(m, "translation").try_into().unwrap());
        out.insert(
            name,
            Frame {
                at: [t[0] + p[0], t[1] + p[1], t[2] + p[2]],
                forward: obj(rotate(&floats(m, "rotation"), [1.0, 0.0, 0.0])),
            },
        );
    }
    out
}

/// The model box (`mode` compression position bounds): x, y, z min and max.
fn mode_box(mode: &Record) -> [[f32; 2]; 3] {
    let Some(Val::Block(c)) = mode.at("compression") else { panic!() };
    let b = floats(&c[0], "position_bounds");
    [[b[0], b[1]], [b[2], b[3]], [b[4], b[5]]]
}

/// A seat's entry tests (`vehi +0x1c8` element `+0x98..+0xa4`).
#[derive(Clone, Copy, Debug)]
struct Entry {
    radius: f32,
    facing: f32,
    look: f32,
    near: f32,
}

impl Entry {
    fn of(seat: &Record) -> Self {
        let f = |n| f32::from_bits(word(seat, n));
        Self {
            radius: f("unknown_98"),
            facing: f("unknown_9c"),
            look: f("unknown_a0"),
            near: f("unknown_a4"),
        }
    }
}

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    dot(&a, &b).clamp(-1.0, 1.0).acos()
}

/// Whether a rider standing on the ground round the vehicle passes
/// `0x000c9040` for an entry marker `m`: from the rider's eye (`0x000cafc0`:
/// the origin, at the rider's feet, raised by the standing eye height
/// `bipd +0x218`), the marker is nearer than `+0x98`; the rider looks at it
/// (`+0xa0`); the marker's forward is within `+0x9c` of the way back to the
/// rider (`0x0011cf50` is the angle between two vectors); the rider's
/// origin is within `+0xa4` of the vehicle's. The vehicle rests on its
/// lowest point and the rider stands clear of its box by its radius.
/// Returns where the first rider admitted stood.
fn admitted(m: Frame, e: Entry, bounds: [[f32; 2]; 3], eye: f32, radius: f32) -> Option<[f32; 3]> {
    let ground = bounds[2][0];
    for step in 0..72 {
        let a = step as f32 * 5f32.to_radians();
        for k in 1..=60 {
            let d = k as f32 * 0.05;
            let (x, y) = (m.at[0] + d * a.cos(), m.at[1] + d * a.sin());
            let clear = x < bounds[0][0] - radius
                || x > bounds[0][1] + radius
                || y < bounds[1][0] - radius
                || y > bounds[1][1] + radius;
            if !clear {
                continue;
            }
            let eye_at = [x, y, ground + eye];
            let to = [m.at[0] - eye_at[0], m.at[1] - eye_at[1], m.at[2] - eye_at[2]];
            let dist = dot(&to, &to).sqrt();
            let dir = to.map(|c| c / dist);
            let origin = dot(&[x, y, ground], &[x, y, ground]).sqrt();
            if e.radius > dist
                && e.look >= 0.0
                && e.facing >= std::f32::consts::PI - angle(m.forward, dir)
                && e.near >= origin
            {
                return Some([x, y, ground]);
            }
        }
    }
    None
}

/// The standing eye height of a rider (`bipd +0x218`).
fn eye_height(bipd: &Record) -> f32 {
    f32::from_bits(word(bipd, "unknown_218"))
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_seat_admits_a_rider_at_its_entry() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let strings = base_map.strings().to_vec();
    // The control: the retail warthog's and ghost's driver and passenger
    // seats admit Halo 2's player.
    let chief = base
        .record(def(b"bipd"), base.find(b"bipd", "objects\\characters\\masterchief\\masterchief_mp").unwrap())
        .unwrap();
    let (eye, radius) = (eye_height(&chief), 0.2);
    assert!((0.5..0.8).contains(&eye), "eye {eye}");
    let mut role_facing = None;
    for (name, seats) in [
        ("objects\\vehicles\\warthog\\warthog", &["warthog_d", "warthog_p"][..]),
        ("objects\\vehicles\\ghost\\ghost", &["ghost_d"][..]),
    ] {
        let v = base.record(def(b"vehi"), base.find(b"vehi", name).unwrap()).unwrap();
        let h = base.record(def(b"hlmt"), reference(&v, "unit.obje.model").unwrap()).unwrap();
        let mode = base.record(def(b"mode"), reference(&h, "render_model").unwrap()).unwrap();
        let (fr, bounds) = (frames(&mode, &strings), mode_box(&mode));
        let Some(Val::Block(all)) = v.at(VEHI_SEATS) else { panic!() };
        for x in all {
            let label = text(x, "label", &strings);
            if !seats.contains(&label.as_str()) {
                continue;
            }
            let m = fr[&text(x, "string_c", &strings)];
            let e = Entry::of(x);
            if label == "warthog_d" {
                role_facing = Some(e.facing);
            }
            let at = admitted(m, e, bounds, eye, radius);
            println!("retail {label}: entry {m:?} {e:?}: admitted at {at:?}");
            assert!(at.is_some(), "retail {label} admits no one: the rider model is wrong");
        }
    }
    let role_facing = role_facing.unwrap();

    // Blood Gulch's vehicles, converted.
    let h1 = h1_level("bloodgulch");
    let mut c = Converter::new(&h1, &base);
    let player = h1
        .tags()
        .iter()
        .find(|t| t.group.0.to_be_bytes() == *b"bipd" && t.name.ends_with("cyborg_mp"))
        .unwrap();
    let id = c.object(player).unwrap();
    let cyborg = defs::from_built(def(b"bipd"), &local(&c.tree, id).unwrap().root).unwrap();
    let eye = eye_height(&cyborg);
    let mut seats = 0;
    for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
        let id = c.object(t).unwrap();
        let v = defs::from_built(def(b"vehi"), &local(&c.tree, id).unwrap().root).unwrap();
        let h = defs::from_built(def(b"hlmt"), &local(&c.tree, reference(&v, "unit.obje.model").unwrap()).unwrap().root).unwrap();
        let mode = defs::from_built(def(b"mode"), &local(&c.tree, reference(&h, "render_model").unwrap()).unwrap().root).unwrap();
        let (fr, bounds) = (frames(&mode, &[]), mode_box(&mode));
        let Some(Val::Block(all)) = v.at(VEHI_SEATS) else { panic!() };
        for x in all {
            if word(x, SEAT_FLAGS) & SEAT_BOARDING != 0 {
                continue;
            }
            let label = text(x, "label", &[]);
            let entry = text(x, "string_c", &[]);
            // An empty entry marker is the object's origin, facing its forward.
            let m = fr.get(&entry).copied().unwrap_or(Frame { at: [0.0; 3], forward: [1.0, 0.0, 0.0] });
            let e = Entry::of(x);
            assert_eq!(word(x, SEAT_ENTRY_FACING), ANY_FACING.to_bits(), "{} {label}", t.name);
            let at = admitted(m, e, bounds, eye, radius);
            println!("{} {label}: entry {entry:?} {m:?} {e:?}: admitted at {at:?}", t.name);
            assert!(at.is_some(), "{} {label}: no rider is admitted", t.name);
            seats += 1;
            // The cause: Halo 1's driver marker faces the warthog's
            // forward, and with the role's 70 degrees a rider beside the
            // door was refused (one ahead of it, looking back, got in).
            if t.name == "vehicles\\warthog\\warthog" && label == "warthog_d" {
                assert!((m.forward[0] - 1.0).abs() < 1e-3, "{m:?}");
                let before = Entry { facing: role_facing, ..e };
                let beside = [m.at[0], m.at[1] + 0.6, bounds[2][0]];
                println!("with the role's facing {role_facing}: admitted at {:?}", admitted(m, before, bounds, eye, radius));
                // A rider square beside the door, looking at the marker:
                // 90 degrees off its forward, out of 70.
                let eye_at = [beside[0], beside[1], beside[2] + eye];
                let to = [m.at[0] - eye_at[0], m.at[1] - eye_at[1], m.at[2] - eye_at[2]];
                let n = dot(&to, &to).sqrt();
                let off = angle(m.forward, to.map(|c| c / n));
                assert!(role_facing < std::f32::consts::PI - off, "{off}");
            }
        }
    }
    assert!(seats >= 9, "{seats} seats");
}

/// The lowest point of the converted vehicle's `mode`.
fn lowest(mode: &Record) -> f32 {
    mode_box(mode)[2][0]
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bounding_centres_sit_inside_the_body() {
    let mut retail = 0;
    for base_name in ["coagulation", "beavercreek"] {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
            let v = base.record(def(b"vehi"), t.0).unwrap();
            let Some(h) = reference(&v, "unit.obje.model") else { continue };
            let h = base.record(def(b"hlmt"), h).unwrap();
            let Some(m) = reference(&h, "render_model") else { continue };
            let mode = base.record(def(b"mode"), m).unwrap();
            let z = floats(&v, "unit.obje.bounding_offset")[2];
            println!("retail {base_name} {}: centre z {z}, lowest {}", t.2, lowest(&mode));
            // No rule holds across retail: the warthog and its turrets
            // centre 0.29 to 0.5 up; the ghost, banshee, scorpion and
            // wraith on their origins (the wraith's 3 cm under its model).
            // The warthog, the converted warthog's counterpart, is the control.
            if t.2.starts_with("objects\\vehicles\\warthog") {
                assert!(z - lowest(&mode) >= 0.1, "retail {}", t.2);
                retail += 1;
            }
        }
    }
    assert!(retail >= 3, "{retail}");

    let mut converted = 0;
    let mut bases = std::collections::HashMap::new();
    for (level, _, base_name) in h1convert::convert::MAPS {
        let base = bases
            .entry(base_name)
            .or_insert_with(|| Base::read(&mut h2_map(base_name)).unwrap());
        let Some(h1) = h1_level_on_disc(level) else {
            println!("{level}: not on the Xbox disc");
            continue;
        };
        let mut c = Converter::new(&h1, base);
        for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
            let u = h1::Unit::read(&h1, t).unwrap();
            // Halo 1 centres every vehicle's sphere on its origin.
            assert_eq!(u.object.bounding_offset, [0.0; 3], "{level} {}", t.name);
            let id = c.object(t).unwrap();
            let v = defs::from_built(def(b"vehi"), &local(&c.tree, id).unwrap().root).unwrap();
            let h = defs::from_built(def(b"hlmt"), &local(&c.tree, reference(&v, "unit.obje.model").unwrap()).unwrap().root).unwrap();
            let mode = defs::from_built(def(b"mode"), &local(&c.tree, reference(&h, "render_model").unwrap()).unwrap().root).unwrap();
            let centre = floats(&v, "unit.obje.bounding_offset");
            let r = float(&v, "unit.obje.bounding_radius");
            let b = mode_box(&mode);
            let corner = (0..3).map(|a| (b[a][1] - b[a][0]).powi(2)).sum::<f32>().sqrt() / 2.0;
            assert!(centre[2] - b[2][0] >= 0.1, "{level} {}: centre {centre:?}, box {b:?}", t.name);
            // The box's middle, and a radius that reaches every vertex but
            // no further than the box's corners.
            for a in 0..3 {
                assert!((centre[a] - (b[a][0] + b[a][1]) / 2.0).abs() < 1e-3, "{level} {}", t.name);
            }
            assert!(r <= corner + 1e-3 && r >= corner / 3f32.sqrt() - 1e-3, "{level} {}: r {r}, corner {corner}", t.name);
            converted += 1;
        }

        if level != "bloodgulch" {
            continue;
        }
        // On Blood Gulch, from where each spawn puts the vehicle: the
        // centre in open space with the floor within the lighting ray.
        let refs = h1.structure_bsps().unwrap();
        let col = h1.structure_bsp(&refs[0]).unwrap().collision_bsps().unwrap().remove(0);
        let spawns = h1convert::convert::halo1::plan_objects(&h1).unwrap();
        let mp = h1.multiplayer().unwrap();
        let mut placed = 0;
        for sp in spawns.iter().filter(|s| s.is_vehicle()) {
            let t = h1.find(h1map::Group::new(b"vehi"), &sp.collection).unwrap();
            let id = c.object(t).unwrap();
            let v = defs::from_built(def(b"vehi"), &local(&c.tree, id).unwrap().root).unwrap();
            let o = floats(&v, "unit.obje.bounding_offset");
            let (s, co) = sp.facing.sin_cos();
            let at = sp.position;
            let centre = [at[0] + co * o[0] - s * o[1], at[1] + s * o[0] + co * o[1], at[2] + o[2]];
            let open = h1convert::query::leaf_at(&col, centre).is_some();
            let start = [centre[0], centre[1], centre[2] + 0.01];
            let floor = h1convert::query::first_hit(&col, start, [start[0], start[1], start[2] - 10.0]);
            println!("{}: spawn {at:?}, centre {centre:?}: open {open}, floor at {floor:?} of 10", sp.collection);
            assert!(open && floor.is_some_and(|f| f > 0.0), "{}: centre {centre:?}", sp.collection);
            placed += 1;
        }
        // Halo 1's sphere sat on the origin, 1 mm over the floor at its
        // placement: a settle of more put the lighting ray and the
        // location's point under the ground.
        for p in &mp.vehicles {
            let up = [p.position[0], p.position[1], p.position[2] + 1.0];
            let floor = h1convert::query::first_hit(&col, up, [up[0], up[1], up[2] - 3.0]).unwrap() as f32;
            let above = p.position[2] - (up[2] - 3.0 * floor);
            assert!((0.0..0.01).contains(&above), "{above}");
        }
        assert_eq!(placed, mp.vehicles.len());
    }
    assert!(converted >= 19, "{converted}");
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_sandbox_spawns_vehicles_above_the_floor() {
    let h1 = h1_level("bloodgulch");
    let refs = h1.structure_bsps().unwrap();
    let col = h1.structure_bsp(&refs[0]).unwrap().collision_bsps().unwrap().remove(0);
    let floor_under = |p: [f32; 3]| {
        let up = [p[0], p[1], p[2] + 1.0];
        let t = h1convert::query::first_hit(&col, up, [up[0], up[1], up[2] - 3.0]).unwrap() as f32;
        up[2] - 3.0 * t
    };
    let mp = h1.multiplayer().unwrap();
    let spawns: Vec<_> = h1convert::convert::halo1::plan_objects(&h1)
        .unwrap()
        .into_iter()
        .filter(|s| s.is_vehicle())
        .collect();
    assert_eq!(spawns.len(), mp.vehicles.len());
    let mut ghosts = 0;
    for (sp, p) in spawns.iter().zip(&mp.vehicles) {
        let depth = h1convert::convert::halo1::vehicle_depth(&h1, &sp.collection).unwrap();
        let floor = floor_under(p.position);
        let was = p.position[2] - depth - floor;
        let now = sp.position[2] - depth - floor;
        println!("{}: depth {depth}, lowest point was {was} over the floor, now {now}", sp.collection);
        assert!((0.0..0.01).contains(&now), "{}: {now}", sp.collection);
        assert_eq!(sp.position[..2], p.position[..2]);
        if sp.collection.contains("ghost") {
            // The cause: the hull began 0.14 into the ground.
            assert!(was < -0.1, "{was}");
            ghosts += 1;
        }
    }
    assert!(ghosts >= 2);
}
