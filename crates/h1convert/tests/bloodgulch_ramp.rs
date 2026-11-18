//! Blood Gulch's blue base ramps, where a converted player fell through:
//! the surfaces there, and the engine's vector test over them in both
//! encodings, densely.
//!
//! ```sh
//! H1_ISO=... cargo test --release -p h1convert --test bloodgulch_ramp -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::surface_query::{self as q, Outcome, Surfaces};
use h1map::H1Map;
use h1map::scenario::FlagKind;

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn bloodgulch() -> H1Map {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map("bloodgulch").cloned().unwrap();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn round_trip(h1: &h1map::bsp::CollisionBsp) -> tags::collision::Bsp {
    let b = h1convert::collision::convert(h1).unwrap();
    const BASE: u32 = 0x8000_0000;
    let bytes = b.encode_blocks(BASE);
    tags::collision::Bsp::decode_structure(&bytes, BASE, 0).unwrap()
}

fn facing(b: &impl Surfaces, s: usize) -> [f32; 4] {
    let (p, flip, _) = b.surface(s).unwrap();
    let p = b.plane(p).unwrap();
    if flip { p.map(|v| -v) } else { p }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_ramps() {
    let h1 = bloodgulch();
    let mp = h1.multiplayer().unwrap();
    for f in &mp.flags {
        if matches!(f.kind, FlagKind::CtfFlag) {
            println!(
                "ctf flag team {} at {:?} facing {}",
                f.team, f.position, f.facing
            );
        }
    }
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    let conv = round_trip(&bsp);
    let blue = mp
        .flags
        .iter()
        .find(|f| matches!(f.kind, FlagKind::CtfFlag) && f.team == 1)
        .unwrap()
        .position;
    // Large sloped, upward-facing surfaces within 25 m of the blue flag.
    let mut ramp = Vec::new();
    for i in 0..bsp.surface_count() {
        let n = facing(&bsp, i);
        let ring = bsp.ring(i);
        let c: [f32; 3] =
            std::array::from_fn(|k| ring.iter().map(|p| p[k]).sum::<f32>() / ring.len() as f32);
        let d = ((c[0] - blue[0]).powi(2) + (c[1] - blue[1]).powi(2)).sqrt();
        let area = {
            let mut a = [0f32; 3];
            for k in 1..ring.len().saturating_sub(1) {
                let (u, v) = (sub(ring[k], ring[0]), sub(ring[k + 1], ring[0]));
                let c = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                for j in 0..3 {
                    a[j] += c[j] / 2.0;
                }
            }
            (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
        };
        if d < 25.0 && n[2] > 0.2 && n[2] < 0.999 && area > 1.0 && c[2] > 0.5 {
            ramp.push(i);
            println!(
                "surface {i}: area {area:.1} n {:?} flags {:#x} ring {:?}",
                &n[..3],
                bsp.surface(i).unwrap().2,
                ring
            );
        }
    }
    // A dense downward grid over the whole base area.
    let (mut rays, mut diff, mut h1_holes, mut conv_holes) = (0, 0, Vec::new(), Vec::new());
    let step = 0.05f32;
    let (x0, y0) = (blue[0] - 20.0, blue[1] - 20.0);
    for ix in 0..(40.0 / step) as i32 {
        for iy in 0..(40.0 / step) as i32 {
            let (x, y) = (x0 + ix as f32 * step, y0 + iy as f32 * step);
            let (a, z) = ([x, y, blue[2] + 12.0], [x, y, blue[2] - 6.0]);
            rays += 1;
            let t1 = q::vector(&bsp, a, z);
            let t2 = q::vector(&conv, a, z);
            let hit = |t: &q::Trace| match t.outcome {
                Outcome::Hit(h) => Some((h.surface, (h.t * 1e4).round() as i32)),
                Outcome::Clear => None,
            };
            if hit(&t1) != hit(&t2) {
                diff += 1;
            }
            if !t1.missed.is_empty() {
                h1_holes.push(([x, y], t1.missed.clone(), hit(&t1)));
            }
            if !t2.missed.is_empty() {
                conv_holes.push(([x, y], t2.missed.clone(), hit(&t2)));
            }
        }
    }
    println!(
        "{rays} rays down over the blue base: {diff} differ, holes: Halo 1 {}, converted {}",
        h1_holes.len(),
        conv_holes.len()
    );
    for h in conv_holes.iter().take(60) {
        println!("  hole {h:?}");
        for &(t, plane, leaf) in &h.1 {
            let z = blue[2] + 12.0 - 18.0 * t;
            let p = [h.0[0], h.0[1], z];
            println!(
                "    at {p:?} plane {plane} {:?} leaf {leaf} {:?}",
                bsp.plane(plane),
                bsp.leaf(leaf)
            );
            let (_, first, count) = bsp.leaf(leaf).unwrap();
            for r in first..first + count {
                let (rp, flip, root) = bsp.reference(r).unwrap();
                let pl = bsp.plane(rp).unwrap();
                let w = q::walk_2d(&bsp, root, q::project(pl, flip, p));
                println!(
                    "      ref plane {rp} flip {flip} -> {w:?} outside {:?}",
                    w.map(|s| q::outside(&bsp, s, p))
                );
            }
            for i in 0..bsp.surface_count() {
                let (sp, ..) = bsp.surface(i).unwrap();
                if sp == plane && q::outside(&bsp, i, p) < 0.5 {
                    println!(
                        "      surface {i} on the plane, outside {} ring {:?}",
                        q::outside(&bsp, i, p),
                        bsp.ring(i)
                    );
                }
            }
        }
    }
    let mut rings = vec![0usize; 64];
    for i in 0..bsp.surface_count() {
        rings[bsp.ring(i).len().min(63)] += 1;
    }
    println!(
        "ring sizes: {:?}",
        rings
            .iter()
            .enumerate()
            .filter(|x| *x.1 > 0)
            .collect::<Vec<_>>()
    );
    // The converted BSP answers as Halo 1's does, hole for hole (on the disc:
    // five, each a point a few millimetres outside its polygon at a seam).
    assert_eq!(diff, 0);
    assert_eq!(h1_holes.len(), conv_holes.len());
    assert!(conv_holes.len() < 16, "{} holes", conv_holes.len());
}

/// The sphere query `0x001ddea0` (with `0x001dde10`'s state): planes the
/// sphere straddles are recorded on a 128-entry stack (state `+0x1c`, no
/// bound check; entry 128 lands on the axis, flip and abort bytes at
/// `+0x21c..+0x220`), back child first with bit 15 set; in a leaf (at most
/// 256 recorded) only references whose plane word equals a recorded entry
/// are walked with the 2D sphere walk `0x001de220` from the centre
/// projected onto the reference's plane: back child when `d <= r`, front
/// when `d >= -r`. Returns the surfaces reached (before the polygon test
/// `0x001de2c0`), the deepest stack and the leaves visited.
struct Sphere {
    surfaces: Vec<usize>,
    deepest: usize,
    leaves: usize,
}

fn sphere(b: &impl Surfaces, c: [f32; 3], r: f32) -> Sphere {
    fn walk2(
        b: &impl Surfaces,
        k: q::Kid2,
        uv: [f32; 2],
        r: f32,
        out: &mut Vec<usize>,
        budget: &mut usize,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        match k {
            q::Kid2::Surface(s) => out.push(s),
            q::Kid2::None => {}
            q::Kid2::Node(n) => {
                let (l, back, front) = b.node_2d(n).unwrap();
                let d = l[0] * uv[0] + l[1] * uv[1] - l[2];
                if r >= d {
                    walk2(b, back, uv, r, out, budget);
                }
                if d >= -r {
                    walk2(b, front, uv, r, out, budget);
                }
            }
        }
    }
    fn go(
        b: &impl Surfaces,
        k: q::Kid,
        c: [f32; 3],
        r: f32,
        stack: &mut Vec<(usize, bool)>,
        out: &mut Sphere,
        budget: &mut usize,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        match k {
            q::Kid::Solid => {}
            q::Kid::Leaf(l) => {
                out.leaves += 1;
                let (_, first, count) = b.leaf(l).unwrap();
                for i in first..first + count {
                    let (rp, flip, root) = b.reference(i).unwrap();
                    if !stack.contains(&(rp, flip)) {
                        continue;
                    }
                    let pl = b.plane(rp).unwrap();
                    let d = pl[0] * c[0] + pl[1] * c[1] + pl[2] * c[2] - pl[3];
                    let p = [c[0] - pl[0] * d, c[1] - pl[1] * d, c[2] - pl[2] * d];
                    let mut bud = 1 << 16;
                    walk2(
                        b,
                        root,
                        q::project(pl, flip, p),
                        r,
                        &mut out.surfaces,
                        &mut bud,
                    );
                }
            }
            q::Kid::Node(n) => {
                let (pi, back, front) = b.node(n).unwrap();
                let p = b.plane(pi).unwrap();
                let d = p[0] * c[0] + p[1] * c[1] + p[2] * c[2] - p[3];
                if d < r && d > -r {
                    stack.push((pi, true));
                    out.deepest = out.deepest.max(stack.len());
                    go(b, back, c, r, stack, out, budget);
                    stack.pop();
                    stack.push((pi, false));
                    go(b, front, c, r, stack, out, budget);
                    stack.pop();
                } else if d <= -r {
                    go(b, back, c, r, stack, out, budget);
                } else {
                    go(b, front, c, r, stack, out, budget);
                }
            }
        }
    }
    let mut out = Sphere {
        surfaces: Vec::new(),
        deepest: 0,
        leaves: 0,
    };
    let mut budget = 1 << 22;
    go(
        b,
        q::Kid::Node(0),
        c,
        r,
        &mut Vec::new(),
        &mut out,
        &mut budget,
    );
    out.surfaces.sort_unstable();
    out.surfaces.dedup();
    out
}

/// Spheres resting on (and slightly into) every upward surface hit by the
/// downward grid over the blue base: does the engine's sphere query reach
/// the surface under them, and how deep does its plane stack go?
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_sphere_queries() {
    let h1 = bloodgulch();
    let mp = h1.multiplayer().unwrap();
    let blue = mp
        .flags
        .iter()
        .find(|f| matches!(f.kind, FlagKind::CtfFlag) && f.team == 1)
        .unwrap()
        .position;
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    let conv = round_trip(&bsp);
    for radius in [0.2f32, 0.5, 1.0, 2.0] {
        let (mut n, mut miss, mut deepest, mut over, mut leaves, mut most) =
            (0, Vec::new(), 0, 0, 0, 0);
        let step = 0.1f32;
        for ix in 0..(40.0 / step) as i32 {
            for iy in 0..(40.0 / step) as i32 {
                let (x, y) = (
                    blue[0] - 20.0 + ix as f32 * step,
                    blue[1] - 20.0 + iy as f32 * step,
                );
                let t = q::vector(&conv, [x, y, blue[2] + 12.0], [x, y, blue[2] - 6.0]);
                let Outcome::Hit(h) = t.outcome else { continue };
                let z = blue[2] + 12.0 - 18.0 * h.t;
                let nrm = facing(&conv, h.surface);
                let c = [
                    x + nrm[0] * radius * 0.8,
                    y + nrm[1] * radius * 0.8,
                    z + nrm[2] * radius * 0.8,
                ];
                let a = sphere(&conv, c, radius);
                let b1 = sphere(&bsp, c, radius);
                n += 1;
                deepest = a.deepest.max(deepest);
                if a.deepest >= 128 {
                    over += 1;
                }
                leaves += usize::from(a.leaves > 256);
                most = most.max(a.surfaces.len());
                if !a.surfaces.contains(&h.surface) || a.surfaces != b1.surfaces {
                    miss.push((
                        [x, y, z],
                        h.surface,
                        a.surfaces.len(),
                        b1.surfaces.contains(&h.surface),
                    ));
                }
            }
        }
        println!(
            "radius {radius}: {n} spheres, {} miss the floor under them, deepest plane stack {deepest} ({over} at 128 or more), {leaves} over 256 leaves, most surfaces {most}",
            miss.len()
        );
        for m in miss.iter().take(10) {
            println!("  {m:?}");
        }
        assert!(miss.is_empty() && deepest < 128 && leaves == 0 && most < 256);
    }
}

/// The first floor straight down, every 0.5 m over 30 m around the blue
/// flag, with the slope of each: where the base's ramps are, if the
/// structure has them.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_height_map() {
    let h1 = bloodgulch();
    let mp = h1.multiplayer().unwrap();
    let blue = mp
        .flags
        .iter()
        .find(|f| matches!(f.kind, FlagKind::CtfFlag) && f.team == 1)
        .unwrap()
        .position;
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    println!(
        "rows y from +15 to -15 m (top = +y), columns x from -15 to +15 m; z in dm, '/' slope 10-40 deg"
    );
    for iy in (-30..=30).rev() {
        let mut line = String::new();
        for ix in -30..=30 {
            let (x, y) = (blue[0] + ix as f32 * 0.5, blue[1] + iy as f32 * 0.5);
            let t = q::vector(&bsp, [x, y, 30.0], [x, y, -10.0]);
            match t.outcome {
                Outcome::Hit(h) => {
                    let z = 30.0 - 40.0 * h.t;
                    let n = facing(&bsp, h.surface);
                    let mark = if n[2] < 0.985 && n[2] > 0.76 {
                        '/'
                    } else {
                        ' '
                    };
                    line.push_str(&format!("{:>3}{mark}", (z * 10.0).round() as i32));
                }
                Outcome::Clear => line.push_str("  - "),
            }
        }
        println!("{line}");
    }
}

/// Halo 1's scenery placed within 20 m of the blue flag, by name: the
/// base's parts that are objects rather than structure.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_scenery() {
    use h1map::scenario::layout;
    let h1 = bloodgulch();
    let mp = h1.multiplayer().unwrap();
    let blue = mp
        .flags
        .iter()
        .find(|f| matches!(f.kind, FlagKind::CtfFlag) && f.team == 1)
        .unwrap()
        .position;
    let scnr = h1.tag_data(h1.scenario()).unwrap();
    let space = h1.tag_space();
    let palette = space
        .block_at(scnr, layout::SCENERY_PALETTE.0, layout::SCENERY_PALETTE.1)
        .unwrap();
    let names: Vec<String> = palette
        .iter()
        .map(|e| {
            h1map::TagRef::read(e, 0)
                .and_then(|r| h1.tag(r.id).ok())
                .map_or_else(|| "?".into(), |t| t.name.clone())
        })
        .collect();
    for e in space
        .block_at(scnr, layout::SCENERY.0, layout::SCENERY.1)
        .unwrap()
    {
        let p = i16::from_le_bytes([e[0], e[1]]);
        let f = |o: usize| f32::from_le_bytes(e[o..o + 4].try_into().unwrap());
        let pos = [f(8), f(12), f(16)];
        let d = ((pos[0] - blue[0]).powi(2) + (pos[1] - blue[1]).powi(2)).sqrt();
        if d < 20.0 {
            let name = usize::try_from(p)
                .ok()
                .and_then(|p| names.get(p))
                .cloned()
                .unwrap_or_default();
            println!(
                "scenery {name} at {pos:?} rotation {:?}",
                [f(0x14), f(0x18), f(0x1c)]
            );
        }
    }
}

/// What Havok's child shape for a structure key (`0x001edd00`: the
/// surface's ring from its first edge, vertices inline) takes as its
/// normal: `0x001edf50` crosses the first three vertices,
/// `(v1 - v0) x (v2 - v0)`. Per surface: the angle between that and the
/// surface's outward plane normal, in degrees (`None` when the three are
/// collinear).
fn shape_normal_angles(b: &impl Surfaces) -> Vec<Option<f32>> {
    (0..b.surface_count())
        .map(|i| {
            let r = b.ring(i);
            if r.len() < 3 {
                return None;
            }
            let (u, v) = (sub(r[1], r[0]), sub(r[2], r[0]));
            let c = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let len = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            let scale = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt()
                * (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if len <= 1e-6 * scale || len == 0.0 {
                return None;
            }
            let n = facing(b, i);
            let d = (c[0] * n[0] + c[1] * n[1] + c[2] * n[2]) / len;
            Some(d.clamp(-1.0, 1.0).acos().to_degrees())
        })
        .collect()
}

fn histogram(a: &[Option<f32>]) -> String {
    let none = a.iter().filter(|x| x.is_none()).count();
    let bins = [1.0f32, 5.0, 30.0, 90.0, 150.0, 175.0, 179.0, 180.1];
    let mut lo = 0.0;
    let mut out = format!("{} surfaces, {none} collinear;", a.len());
    for hi in bins {
        let n = a.iter().flatten().filter(|&&x| x >= lo && x < hi).count();
        out += &format!(" [{lo},{hi}) {n}");
        lo = hi;
    }
    out
}

/// Havok's normal of each surface shape against the plane: converted Blood
/// Gulch against the retail Halo 2 controls in `$H2_MAPS` (default
/// coagulation and lockout).
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn surface_shape_normals() {
    let h1 = bloodgulch();
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    let conv = round_trip(&bsp);
    let a = shape_normal_angles(&conv);
    println!("bloodgulch converted: {}", histogram(&a));
    println!(
        "bloodgulch Halo 1: {}",
        histogram(&shape_normal_angles(&bsp))
    );
    // The base's surfaces (x 34..47, y -86..-72) that Havok would see wrong.
    for (i, x) in a.iter().enumerate() {
        let ring = conv.ring(i);
        let c: [f32; 3] =
            std::array::from_fn(|k| ring.iter().map(|p| p[k]).sum::<f32>() / ring.len() as f32);
        if c[0] > 34.0 && c[0] < 47.0 && c[1] > -86.0 && c[1] < -72.0 && !x.is_some_and(|d| d < 5.0)
        {
            println!(
                "  base surface {i}: angle {x:?} n {:?} ring {ring:?}",
                &facing(&conv, i)[..3]
            );
        }
    }
    assert!(
        a.iter().all(|x| x.is_some_and(|d| d < 1.0)),
        "a surface shape's normal is off its plane"
    );
    let maps = std::env::var("H2_MAPS").unwrap_or_else(|_| "coagulation,lockout".into());
    for (name, b) in h2_bsps(&maps) {
        println!("retail {name}: {}", histogram(&shape_normal_angles(&b)));
    }
}

fn h2_bsps(only: &str) -> Vec<(String, tags::collision::Bsp)> {
    use map::{FourCC, Map};
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
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
        if !only.split(',').any(|x| x == stem) {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut m = Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let bsps: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| (t.id, t.location.unwrap().address))
            .collect();
        for (id, address) in bsps {
            let b = m.read_tag_bytes(id).unwrap();
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            if u(0x14) != 1 {
                continue;
            }
            let at = (u(0x18) - address) as usize;
            if let Ok(c) = tags::collision::Bsp::decode_structure(&b, address, at) {
                out.push((stem.to_owned(), c));
            }
        }
    }
    out
}

/// Every surface a vertical line meets, top down: (z, surface, outward
/// normal z), by restarting the vector test 1 mm past each hit.
fn column(b: &impl Surfaces, x: f32, y: f32) -> Vec<(f32, usize, f32)> {
    let mut out = Vec::new();
    let mut top = 10.0f32;
    while top > -3.0 && out.len() < 16 {
        let t = q::vector(b, [x, y, top], [x, y, -3.0]);
        let Outcome::Hit(h) = t.outcome else { break };
        let z = top + (-3.0 - top) * h.t;
        out.push((z, h.surface, facing(b, h.surface)[2]));
        top = z - 1e-3;
    }
    out
}

/// Cross sections through the +x ramp of the blue base: along x at the
/// ramp's y, and along y at its middle.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_ramp_sections() {
    let h1 = bloodgulch();
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    for y in [-80.6f32, -80.1, -79.6, -79.1, -78.6] {
        println!("y {y}");
        for i in 0..=30 {
            let x = 41.0 + 0.2 * i as f32;
            let c: Vec<String> = column(&bsp, x, y)
                .iter()
                .map(|(z, s, n)| format!("{z:.2}({s},{n:.2})"))
                .collect();
            println!("  x {x:.1}: {}", c.join(" "));
        }
    }
    // The outside ramp (y -79.1, x 45.2 up to 42.8, z 0.18 to 1.63) is one
    // surface whose slope is under both games' steepest walkable ground:
    // Halo 1's cyborg 45 degrees (cosine 0.7071 at bipd +0x4d0), Halo 2's
    // Chief 50 degrees (cosine 0.6428 at bipd +0x2b8, which the converted
    // cyborg holds as its role's word).
    let ramp: Vec<_> = (0..=10)
        .map(|i| {
            column(&bsp, 43.0 + 0.2 * i as f32, -79.1)
                .last()
                .copied()
                .unwrap()
        })
        .collect();
    assert!(ramp.iter().all(|r| r.1 == ramp[0].1), "{ramp:?}");
    let slope = ramp[0].2.acos().to_degrees();
    println!("ramp surface {}: slope {slope:.1} degrees", ramp[0].1);
    assert!(ramp[0].2 > 0.7071 && ramp[0].2 > 0.6428, "{slope}");
}

/// The character-physics words side by side: Halo 1's cyborg (`bipd`
/// `+0x400..+0x4d0`) and the Halo 2 Chief (`bipd +0x264..+0x314`), as
/// floats (and hex), to place Halo 1's ground words against Halo 2's.
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn character_physics_words() {
    use map::{FourCC, Map};
    let h1 = bloodgulch();
    let t = h1
        .tags()
        .iter()
        .filter(|t| {
            [u32::from_be_bytes(*b"bipd"), u32::from_le_bytes(*b"bipd")].contains(&t.group.0)
        })
        .filter(|t| {
            t.name == "characters\\cyborg_mp\\cyborg_mp" || t.name == "characters\\cyborg\\cyborg"
        })
        .max_by_key(|t| t.data.as_ref().map_or(0, |d| d.len()))
        .unwrap()
        .clone();
    let b = h1.tag_data(&t).unwrap();
    println!("Halo 1 {} ({:?})", t.name, t.group);
    for o in (0x2f0..0x4f0).step_by(4) {
        let w = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        println!("  h1 +{o:#x}: {:>12.6} {w:#010x}", f32::from_bits(w));
    }
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with("coagulation.map"))
        .unwrap();
    let mut m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let id = m
        .tags()
        .iter()
        .find(|t| {
            t.group == FourCC(*b"bipd")
                && t.name.as_deref() == Some("objects\\characters\\masterchief\\masterchief")
        })
        .unwrap()
        .id;
    let b = m.read_tag_bytes(id).unwrap();
    for o in (0x1c8..0x314).step_by(4) {
        let w = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        println!("  h2 +{o:#x}: {:>12.6} {w:#010x}", f32::from_bits(w));
    }
}

/// Whether `p` (on or near the surface's plane) lies inside the ring's
/// polygon, within `tol`, projected on the plane's dominant axis.
fn inside_ring(ring: &[[f32; 3]], n: [f32; 4], p: [f32; 3], tol: f32) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let drop = (0..3)
        .max_by(|&a, &b| n[a].abs().total_cmp(&n[b].abs()))
        .unwrap();
    let (u, v) = match drop {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    // Signed distance from each edge, inward positive for either winding.
    let mut area = 0.0f32;
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        area += a[u] * b[v] - b[u] * a[v];
    }
    let sign = area.signum();
    (0..ring.len()).all(|k| {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let (ex, ey) = (b[u] - a[u], b[v] - a[v]);
        let len = (ex * ex + ey * ey).sqrt().max(1e-9);
        sign * (ex * (p[v] - a[v]) - ey * (p[u] - a[u])) / len >= -tol
    })
}

/// What Havok sees of the +x ramp, where the live repro (recording
/// `bg-ramp-fall`) lost the ground: running up at y -79.33 and -78.92, the
/// ramp's contact goes away at x 43.6 and the capsule sinks through the
/// ramp's top metre (contacts then show the ramp's back face). The game's
/// vector test still finds the ramp there, so the question is Havok's own
/// view: the surface's ring polygon (the shape `0x001edd00` builds by the
/// same edge walk), its flags and breakable byte (the key filter
/// `0x001ef451` drops breakable surfaces whose bit is clear), and the
/// structure MOPP's keys for a capsule-sized box at each step.
///
/// Also, over the whole base, every point the vector test puts on a surface
/// must lie inside that surface's ring, in Halo 1 and converted alike.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_base_ramp_havok_view() {
    let h1 = bloodgulch();
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    let conv = round_trip(&bsp);
    let code = mopp::Code::parse(&mopp::build_structure(&conv)).unwrap();

    // Rings: converted against Halo 1, every surface.
    let mut ring_diffs = Vec::new();
    for i in 0..bsp.surface_count() {
        let (a, b) = (bsp.ring(i), conv.ring(i));
        let same = a.len() == b.len()
            && a.iter()
                .zip(&b)
                .all(|(p, q)| (0..3).all(|k| (p[k] - q[k]).abs() < 1e-3));
        if !same {
            ring_diffs.push(i);
        }
    }
    println!(
        "{} of {} surface rings differ from Halo 1's: {:?}",
        ring_diffs.len(),
        bsp.surface_count(),
        &ring_diffs[..ring_diffs.len().min(20)]
    );

    // The surfaces around the ramp, as Havok gets them.
    for i in 0..conv.surface_count() {
        let ring = conv.ring(i);
        if ring.is_empty() {
            continue;
        }
        let c: [f32; 3] =
            std::array::from_fn(|k| ring.iter().map(|p| p[k]).sum::<f32>() / ring.len() as f32);
        if c[0] > 42.0 && c[0] < 46.0 && c[1] > -80.2 && c[1] < -78.0 && c[2] > -0.5 && c[2] < 2.0 {
            let sf = conv.surfaces[i];
            println!(
                "surface {i}: n {:?} flags {:#x} breakable {:#x} first edge {} ring ({}) {:?}",
                &facing(&conv, i)[..3],
                sf.flags,
                sf.breakable_surface,
                sf.first_edge,
                ring.len(),
                ring
            );
        }
    }

    // Up the ramp along the repro's lines.
    let mut problems = Vec::new();
    for y in [-79.33f32, -79.1, -78.92] {
        println!("up the ramp at y {y}");
        for i in 0..=46 {
            let x = 45.15 - 0.05 * i as f32;
            let Some(&(z, s, nz)) = column(&conv, x, y).iter().find(|c| c.2 > 0.8 && c.2 < 0.9)
            else {
                problems.push(format!("({x:.2},{y}): no ramp in the column"));
                continue;
            };
            let n = facing(&conv, s);
            let ring = conv.ring(s);
            let inside = inside_ring(&ring, n, [x, y, z], 0.01);
            let sf = conv.surfaces[s];
            let filtered = sf.flags & 8 != 0 && sf.breakable_surface != 0xff;
            let aabb = mopp::Aabb {
                min: [x - 0.3, y - 0.3, z - 0.2],
                max: [x + 0.3, y + 0.3, z + 1.8],
            };
            let keys = mopp::query_aabb(&code, &aabb).unwrap();
            let keyed = keys.contains(&(0x2000_0000 | s as u32));
            let tiny = mopp::query_sphere(&code, [x, y, z], 0.01).unwrap();
            let keyed_tiny = tiny.contains(&(0x2000_0000 | s as u32));
            println!(
                "  x {x:.2}: z {z:.3} surface {s} (nz {nz:.3}) in ring {inside} mopp box {keyed} sphere {keyed_tiny} filtered {filtered}"
            );
            if !inside || !keyed || !keyed_tiny || filtered {
                problems.push(format!(
                    "({x:.2},{y},{z:.3}) surface {s}: in ring {inside}, mopp box {keyed}, sphere {keyed_tiny}, filtered {filtered}"
                ));
            }
        }
    }

    // Every vector-test hit over the base lies in its surface's ring.
    let mut outside = Vec::new();
    hits_in_rings("halo1", &bsp, &mut outside);
    hits_in_rings("converted", &conv, &mut outside);
    for o in &outside {
        println!("  outside: {o}");
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Walks a 5 cm grid of vertical lines over the blue base, every hit top
/// down, and notes the hits that fall outside their surface's ring.
fn hits_in_rings(name: &str, b: &impl Surfaces, outside: &mut Vec<String>) {
    let (mut n, mut out) = (0, 0);
    for ix in 0..=240 {
        for iy in 0..=240 {
            let (x, y) = (33.0 + 0.05 * ix as f32, -85.0 + 0.05 * iy as f32);
            let mut top = 6.0f32;
            for _ in 0..8 {
                let t = q::vector(b, [x, y, top], [x, y, -3.0]);
                let Outcome::Hit(h) = t.outcome else { break };
                let z = top + (-3.0 - top) * h.t;
                n += 1;
                if !inside_ring(&b.ring(h.surface), facing(b, h.surface), [x, y, z], 0.02) {
                    out += 1;
                    if outside.len() < 40 {
                        outside.push(format!(
                            "{name} ({x:.2},{y:.2},{z:.3}) surface {}",
                            h.surface
                        ));
                    }
                }
                top = z - 1e-3;
            }
        }
    }
    println!("{name}: {n} hits over the base, {out} outside their surface's ring");
}
