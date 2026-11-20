//! Every Halo 1 structure collision BSP (the nineteen multiplayer maps and
//! the ten campaign levels on `$H1_ISO`) asked the same questions in both
//! encodings: Halo 1's, as read from the map, and the converted Halo 2 one,
//! encoded as a structure BSP holds it and read back with
//! `tags::collision::Bsp::decode_structure`. The questions are the engine's
//! (`h1convert::surface_query`): which leaf holds a point, and what a
//! vector test meets: rays straight down onto every floor, into every wall
//! and ceiling, along seams (from one surface's edge midpoint), and random
//! ones. Any disagreement is a conversion bug.
//!
//! The same model is also run on Halo 1's data alone and on every retail
//! Halo 2 structure BSP (`$H2_DISC`) as a control: how often a ray into a
//! surface finds no surface where it enters solid (the engine lets it
//! through), and how often the 2D tree of a leaf's reference takes a
//! surface's own centroid to that surface. These say whether Halo 1's data
//! follows the Halo 2 engine's projection and side rules.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1convert --test it collision_engine:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::surface_query::{self as q, Outcome, Surfaces};
use h1map::H1Map;
use map::{FourCC, Map};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Every Halo 1 map's collision BSPs: (map, index, BSP).
fn h1_bsps() -> Vec<(String, usize, h1map::bsp::CollisionBsp)> {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let only: Option<Vec<String>> = std::env::var("H1_MAPS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let names = h1convert::convert::MAPS
        .iter()
        .map(|m| m.0)
        .chain(h1map::disc::CAMPAIGN_LEVELS);
    let mut out = Vec::new();
    for name in names {
        if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == name)) {
            continue;
        }
        let Some(e) = d.map(name).cloned() else {
            continue;
        };
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let mut k = 0;
        for r in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&r).unwrap();
            for c in s.collision_bsps().unwrap() {
                out.push((name.to_owned(), k, c));
                k += 1;
            }
        }
    }
    out
}

/// The converted BSP as a structure BSP holds it, read back.
fn round_trip(h1: &h1map::bsp::CollisionBsp) -> tags::collision::Bsp {
    let b = h1convert::collision::convert(h1).unwrap();
    const BASE: u32 = 0x8000_0000;
    // The eight block fields, then the blocks: a structure's element.
    let bytes = b.encode_blocks(BASE);
    tags::collision::Bsp::decode_structure(&bytes, BASE, 0).unwrap()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn add(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i] * k)
}

fn centroid(ring: &[[f32; 3]]) -> [f32; 3] {
    let n = ring.len().max(1) as f32;
    std::array::from_fn(|i| ring.iter().map(|p| p[i]).sum::<f32>() / n)
}

/// A surface's plane with its flip applied (pointing out of solid).
fn facing(b: &impl Surfaces, s: usize) -> [f32; 4] {
    let (p, flip, _) = b.surface(s).unwrap();
    let p = b.plane(p).unwrap();
    if flip { p.map(|v| -v) } else { p }
}

/// The rays: (from, to). Into every surface (up to `cap`) from 25 cm out
/// along its facing normal to 25 cm in, at its centroid and at each edge
/// midpoint pulled 1 cm toward the centroid; straight down onto every
/// surface facing up; and `random` segments in the vertex box.
fn rays(b: &impl Surfaces, cap: usize, random: usize, seed: u64) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    let n = b.surface_count();
    let step = (n / cap.max(1)).max(1);
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for s in 0..n {
        let ring = b.ring(s);
        for p in &ring {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        if s % step != 0 || ring.len() < 3 {
            continue;
        }
        let pl = facing(b, s);
        let nrm = [pl[0], pl[1], pl[2]];
        let c = centroid(&ring);
        let mut at = vec![c];
        for i in 0..ring.len() {
            let m = add(ring[i], ring[(i + 1) % ring.len()], 1.0).map(|v| v / 2.0);
            let to_c: [f32; 3] = std::array::from_fn(|k| c[k] - m[k]);
            let len = to_c.iter().map(|v| v * v).sum::<f32>().sqrt();
            if len > 0.02 {
                at.push(add(m, to_c, 0.01 / len));
            }
        }
        for p in at {
            out.push((add(p, nrm, 0.25), add(p, nrm, -0.25)));
            if nrm[2] > 0.3 {
                out.push((add(p, [0.0, 0.0, 1.0], 0.5), add(p, [0.0, 0.0, 1.0], -0.5)));
            }
        }
    }
    let mut r = Rng(seed | 1);
    for _ in 0..random {
        let mut p = || std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * r.next());
        let (a, z) = (p(), p());
        out.push((a, z));
    }
    out
}

#[derive(Default, Debug)]
struct Tally {
    rays: usize,
    hits: usize,
    /// Rays that entered solid somewhere with no surface found there.
    fell: usize,
    /// Rays into a surface (the first half of [`rays`]) that met nothing.
    centre_misses: usize,
    refs_2d: usize,
    refs_2d_wrong: usize,
    /// Leaves whose flag bit 0 disagrees with "a surface its references
    /// reach is two-sided" (surface flag bit 0).
    leaf_flag_wrong: usize,
    leaves: usize,
    /// Surfaces with each flag bit set.
    surface_bits: [usize; 8],
    surfaces: usize,
    /// Leaves with each flag bit set.
    leaf_bits: [usize; 16],
    /// A leaf's references whose (plane, flip) is not a node on the leaf's
    /// path with flip = the leaf on the plane's back: the volume query
    /// (`0x001ddea0`) matches a reference only against the planes it
    /// straddled on the way down, flip for the back child.
    refs_off_path: usize,
    refs: usize,
    /// The most vertices round one surface (Havok builds each surface as
    /// one polygon shape, `0x001edd00`).
    max_ring: usize,
}

/// The model on one BSP alone.
fn tally(b: &impl Surfaces, cap: usize, random: usize) -> Tally {
    let mut t = Tally::default();
    for (a, z) in rays(b, cap, random, 0x5eed) {
        let tr = q::vector(b, a, z);
        t.rays += 1;
        if matches!(tr.outcome, Outcome::Hit(_)) {
            t.hits += 1;
        }
        if !tr.missed.is_empty() {
            t.fell += 1;
        }
    }
    // Each surface a leaf's reference tree reaches, found at its centroid.
    for i in 0..b.surface_count() {
        let (_, _, f) = b.surface(i).unwrap();
        t.surfaces += 1;
        t.max_ring = t.max_ring.max(b.ring(i).len());
        for k in 0..8 {
            t.surface_bits[k] += usize::from(f >> k & 1);
        }
    }
    let paths = leaf_paths(b);
    let mut leaf = 0;
    while let Some((flags, first, count)) = b.leaf(leaf) {
        for r in first..first + count {
            let Some((plane, flip, _)) = b.reference(r) else {
                continue;
            };
            t.refs += 1;
            if !paths.get(leaf).is_some_and(|p| p.contains(&(plane, flip))) {
                t.refs_off_path += 1;
            }
        }
        t.leaves += 1;
        for k in 0..16 {
            t.leaf_bits[k] += (flags >> k & 1) as usize;
        }
        let mut two_sided = false;
        for r in first..first + count {
            let Some((plane, flip, root)) = b.reference(r) else {
                continue;
            };
            let pl = b.plane(plane).unwrap();
            let mut stack = vec![root];
            let mut seen = 0;
            while let Some(k) = stack.pop() {
                seen += 1;
                if seen > 1 << 16 {
                    break;
                }
                match k {
                    q::Kid2::Node(n) => {
                        let (_, x, y) = b.node_2d(n).unwrap();
                        stack.push(x);
                        stack.push(y);
                    }
                    q::Kid2::Surface(s) => {
                        two_sided |= b.surface(s).unwrap().2 & 1 != 0;
                        let c = centroid(&b.ring(s));
                        t.refs_2d += 1;
                        if q::walk_2d(b, root, q::project(pl, flip, c)) != Some(s) {
                            t.refs_2d_wrong += 1;
                        }
                    }
                    q::Kid2::None => {}
                }
            }
        }
        if two_sided != (flags & 1 != 0) {
            t.leaf_flag_wrong += 1;
        }
        leaf += 1;
    }
    t
}

/// Each leaf's path from the root: (plane, the leaf on its back side).
fn leaf_paths(b: &impl Surfaces) -> Vec<Vec<(usize, bool)>> {
    let mut out: Vec<Vec<(usize, bool)>> = Vec::new();
    let mut stack = vec![(q::Kid::Node(0), Vec::new())];
    if b.node_count() == 0 {
        return out;
    }
    let mut budget = 4 * b.node_count() + 16;
    while let Some((k, path)) = stack.pop() {
        budget = match budget.checked_sub(1) {
            Some(v) => v,
            None => break,
        };
        match k {
            q::Kid::Solid => {}
            q::Kid::Leaf(l) => {
                if out.len() <= l {
                    out.resize(l + 1, Vec::new());
                }
                out[l].extend(path);
            }
            q::Kid::Node(n) => {
                let Some((plane, back, front)) = b.node(n) else {
                    continue;
                };
                let mut pb = path.clone();
                pb.push((plane, true));
                let mut pf = path;
                pf.push((plane, false));
                stack.push((back, pb));
                stack.push((front, pf));
            }
        }
    }
    out
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn converted_collision_answers_as_halo_1s() {
    let mut total = (0usize, 0usize, 0usize);
    let mut bad = Vec::new();
    for (name, k, h1) in h1_bsps() {
        let h2 = round_trip(&h1);
        assert_eq!(h2.surfaces.len(), h1.surfaces.len());
        // Points: the leaf (or solid) holding each.
        let mut r = Rng(0xc011_1510 + k as u64);
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &h1.vertices {
            for a in 0..3 {
                lo[a] = lo[a].min(v.point[a]);
                hi[a] = hi[a].max(v.point[a]);
            }
        }
        let mut points = 0;
        for _ in 0..20_000 {
            let p: [f32; 3] = std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * r.next());
            points += 1;
            let (x, y) = (q::locate(&h1, p), q::locate(&h2, p));
            if x != y {
                bad.push(format!("{name} bsp {k}: point {p:?}: {x:?} vs {y:?}"));
            }
        }
        let rays = rays(&h1, 4000, 20_000, 0xbeef + k as u64);
        for (a, z) in &rays {
            let (x, y) = (q::vector(&h1, *a, *z), q::vector(&h2, *a, *z));
            if x != y {
                bad.push(format!(
                    "{name} bsp {k}: ray {a:?} -> {z:?}: {x:?} vs {y:?}"
                ));
            }
        }
        println!(
            "{name} bsp {k}: {} surfaces, {points} points, {} rays agree",
            h1.surfaces.len(),
            rays.len()
        );
        total.0 += 1;
        total.1 += points;
        total.2 += rays.len();
    }
    println!(
        "{} BSPs, {} points, {} rays; {} disagreements",
        total.0,
        total.1,
        total.2,
        bad.len()
    );
    assert!(total.0 >= 29, "every map's BSP: {}", total.0);
    assert!(bad.is_empty(), "{:#?}", &bad[..bad.len().min(20)]);
}

/// Every retail structure BSP's collision.
fn h2_bsps() -> Vec<(String, tags::collision::Bsp)> {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let only: Option<Vec<String>> = std::env::var("H2_MAPS")
        .ok()
        .map(|s| s.split(',').map(str::to_owned).collect());
    let mut out = Vec::new();
    for e in iso.entries().unwrap() {
        let Some(stem) = e
            .path
            .rsplit(['/', '\\'])
            .next()
            .and_then(|f| f.strip_suffix(".map"))
        else {
            continue;
        };
        if only.as_ref().is_some_and(|o| !o.iter().any(|x| x == stem)) {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        let bsps: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| {
                (
                    t.id,
                    t.location.unwrap().address,
                    t.name.clone().unwrap_or_default(),
                )
            })
            .collect();
        for (id, address, name) in bsps {
            let b = m.read_tag_bytes(id).unwrap();
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            if u(0x14) != 1 {
                continue;
            }
            let at = (u(0x18) - address) as usize;
            if let Ok(c) = tags::collision::Bsp::decode_structure(&b, address, at) {
                out.push((format!("{stem}: {name}"), c));
            }
        }
    }
    out
}

fn report(what: &str, t: &Tally) {
    println!(
        "{what}: {} rays, {} hit, {} fell through ({:.3} %), 2D refs {} of {} wrong ({:.3} %)",
        t.rays,
        t.hits,
        t.fell,
        100.0 * t.fell as f64 / t.rays.max(1) as f64,
        t.refs_2d_wrong,
        t.refs_2d,
        100.0 * t.refs_2d_wrong as f64 / t.refs_2d.max(1) as f64,
    );
    println!(
        "  2D references off their leaf's path (or the wrong side): {} of {}",
        t.refs_off_path, t.refs
    );
    println!(
        "  leaves {} (flag bit 0 disagrees on {}), leaf bits {:?}; surfaces {}, bits {:?}, most vertices {}",
        t.leaves, t.leaf_flag_wrong, t.leaf_bits, t.surfaces, t.surface_bits, t.max_ring
    );
}

fn sum(a: &mut Tally, b: &Tally) {
    a.rays += b.rays;
    a.hits += b.hits;
    a.fell += b.fell;
    a.centre_misses += b.centre_misses;
    a.refs_2d += b.refs_2d;
    a.refs_2d_wrong += b.refs_2d_wrong;
    a.leaf_flag_wrong += b.leaf_flag_wrong;
    a.max_ring = a.max_ring.max(b.max_ring);
    a.refs_off_path += b.refs_off_path;
    a.refs += b.refs;
    a.leaves += b.leaves;
    a.surfaces += b.surfaces;
    for k in 0..8 {
        a.surface_bits[k] += b.surface_bits[k];
    }
    for k in 0..16 {
        a.leaf_bits[k] += b.leaf_bits[k];
    }
}

#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn halo_1_data_follows_the_halo_2_engine_rules() {
    let mut h2 = Tally::default();
    for (name, b) in h2_bsps() {
        let t = tally(&b, 2000, 2000);
        if std::env::var("VERBOSE").is_ok() {
            report(&name, &t);
        }
        sum(&mut h2, &t);
    }
    report("Halo 2 retail", &h2);
    let mut h1 = Tally::default();
    let mut conv = Tally::default();
    for (name, k, b) in h1_bsps() {
        let t = tally(&b, 2000, 2000);
        report(&format!("{name} bsp {k}"), &t);
        sum(&mut h1, &t);
        sum(&mut conv, &tally(&round_trip(&b), 2000, 2000));
    }
    report("Halo 1", &h1);
    report("Halo 1 converted", &conv);
    let rate = |t: &Tally| t.refs_2d_wrong as f64 / t.refs_2d.max(1) as f64;
    let fell = |t: &Tally| t.fell as f64 / t.rays.max(1) as f64;
    assert!(
        rate(&h1) <= rate(&h2) * 2.0 + 1e-4,
        "2D projection rules differ"
    );
    assert!(
        fell(&conv) <= fell(&h2) * 2.0 + 1e-4,
        "converted rays fall through"
    );
}

/// The structure's MOPP code over the converted BSP returns every surface
/// to a small sphere at its centroid and at each of its corners: Havok
/// sees every surface where it is.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn converted_mopp_returns_every_surface_where_it_is() {
    let (mut queries, mut misses) = (0usize, Vec::new());
    for (name, k, h1) in h1_bsps() {
        let h2 = round_trip(&h1);
        let code = mopp::Code::parse(&mopp::build_structure(&h2)).unwrap();
        for s in 0..h2.surfaces.len() {
            let ring = h2.ring(s);
            let key = 0x2000_0000 | s as u32;
            for p in ring.iter().copied().chain([centroid(&ring)]) {
                queries += 1;
                let keys = mopp::query_sphere(&code, p, 0.01).unwrap();
                if !keys.contains(&key) {
                    misses.push(format!("{name} bsp {k}: surface {s} at {p:?}"));
                }
            }
        }
    }
    println!("{queries} sphere queries, {} misses", misses.len());
    assert!(misses.is_empty(), "{:#?}", &misses[..misses.len().min(20)]);
}
