//! Relate retail pathfinding data to collision: analyze <map> [bsp]
#[path = "common/disc.rs"]
mod disc;
use anyhow::Result;
use pathfind::geom::*;
use std::collections::{BTreeMap, HashMap};
use tags::pathfinding::RefTarget;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let mut map = disc::open_map(&a[1])?;
    let idx: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(0);
    let (bsp, pf, _, _) = disc::load_bsp(&mut map, idx)?;
    let pf = pf.expect("no pathfinding");
    println!(
        "coll validate: {:?}",
        bsp.validate(usize::MAX).iter().take(5).collect::<Vec<_>>()
    );
    let v = pf.validate(Some(bsp.surfaces.len()));
    println!(
        "pf validate: {} problems {:?}",
        v.len(),
        v.iter().take(8).collect::<Vec<_>>()
    );
    println!(
        "sectors {} links {} refs {} nodes {} verts {} hints {} objrefs {}",
        pf.sectors.len(),
        pf.links.len(),
        pf.refs.len(),
        pf.bsp2d_nodes.len(),
        pf.vertices.len(),
        pf.hints.len(),
        pf.object_refs.len()
    );
    // vertices shared with collision?
    let q = |p: [f32; 3]| {
        (
            (p[0] * 100.0).round() as i64,
            (p[1] * 100.0).round() as i64,
            (p[2] * 100.0).round() as i64,
        )
    };
    let cv: HashMap<_, usize> = bsp
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| (q(v.point), i))
        .collect();
    let shared = pf
        .vertices
        .iter()
        .filter(|p| cv.contains_key(&q(**p)))
        .count();
    println!(
        "pf vertices at a collision vertex (1cm): {shared}/{}",
        pf.vertices.len()
    );
    // ref targets vs surface normal
    let mut hist: BTreeMap<(String, i32), usize> = BTreeMap::new();
    let mut per_sector: HashMap<u32, usize> = HashMap::new();
    let mut surf_poly = Vec::new();
    for (s, surf) in bsp.surfaces.iter().enumerate() {
        let ring: Vec<V3> = bsp
            .surface_ring(s)
            .iter()
            .map(|&i| bsp.vertices[i].point)
            .collect();
        let pl = bsp.planes[(surf.plane & 0x7fff) as usize].0;
        let flip = surf.plane & 0x8000 != 0;
        let nz = if flip { -pl[2] } else { pl[2] };
        let nw = newell(&ring);
        let kind = match pf.refs[s].target() {
            RefTarget::None => "none".to_string(),
            RefTarget::Sector(x) => {
                *per_sector.entry(x).or_default() += 1;
                "sector".into()
            }
            RefTarget::Node(_) => "node".into(),
        };
        let bucket = (nz * 10.0).floor() as i32;
        *hist.entry((kind, bucket)).or_default() += 1;
        let agree = dot(nw, [pl[0], pl[1], pl[2]]) * if flip { -1.0 } else { 1.0 };
        surf_poly.push((ring, nz, agree, surf.flags));
    }
    println!("ref kind by floor(10*nz): {hist:?}");
    let mut spc: BTreeMap<usize, usize> = BTreeMap::new();
    for c in per_sector.values() {
        *spc.entry(*c).or_default() += 1;
    }
    println!(
        "surfaces per directly-referenced sector: {spc:?}; sectors directly referenced {}",
        per_sector.len()
    );
    let agree = surf_poly.iter().filter(|s| s.2 > 0.0).count();
    println!(
        "ring winding agrees with (flipped) plane: {agree}/{}",
        surf_poly.len()
    );
    // surface flag bits vs ref kind
    let mut fl: BTreeMap<(u8, bool), usize> = BTreeMap::new();
    for (s, sp) in surf_poly.iter().enumerate() {
        *fl.entry((sp.3, pf.refs[s] != tags::pathfinding::Ref::NONE))
            .or_default() += 1;
    }
    println!("surface flags x has-ref: {fl:?}");
    // sector polygons
    let mut convex = (0, 0, 0);
    let mut sizes: BTreeMap<usize, usize> = BTreeMap::new();
    let mut sflags: BTreeMap<u16, usize> = BTreeMap::new();
    let mut area = 0.0f64;
    for i in 0..pf.sectors.len() {
        *sflags.entry(pf.sectors[i].flags).or_default() += 1;
        let Some(poly) = pf.sector_polygon(i) else {
            continue;
        };
        *sizes.entry(poly.len()).or_default() += 1;
        let p: Vec<V3> = poly.iter().map(|&k| pf.vertices[k]).collect();
        let sa = signed_area_xy(&p);
        area += sa.abs() as f64;
        if is_convex_xy(&p, true) {
            convex.0 += 1
        } else if is_convex_xy(&p, false) {
            convex.1 += 1
        } else {
            convex.2 += 1
        }
    }
    println!("sector polygon sizes {sizes:?}");
    println!("sector convex ccw/cw/neither {convex:?}; xy area {area:.0}");
    println!("sector flags {sflags:?}");
    let mut lflags: BTreeMap<(u16, bool), usize> = BTreeMap::new();
    for l in &pf.links {
        *lflags
            .entry((
                l.flags,
                l.left_sector != u16::MAX && l.right_sector != u16::MAX,
            ))
            .or_default() += 1;
    }
    println!("link flags x two-sided {lflags:?}");
    // walkable surface area vs sector area
    let mut walk_area = 0.0f64;
    let mut ref_area = 0.0f64;
    for (s, sp) in surf_poly.iter().enumerate() {
        let a = signed_area_xy(&sp.0).abs() as f64;
        if pf.refs[s] != tags::pathfinding::Ref::NONE {
            ref_area += a
        }
        if sp.1 > 0.7 {
            walk_area += a
        }
    }
    println!("xy area: surfaces with ref {ref_area:.0}, surfaces nz>0.7 {walk_area:.0}");
    // point-in-sector via refs: centroid of each surface
    let (mut ok, mut bad, mut none) = (0, 0, 0);
    for (s, sp) in surf_poly.iter().enumerate() {
        if pf.refs[s] == tags::pathfinding::Ref::NONE {
            continue;
        }
        let c = centroid(&sp.0);
        match pf.sector_for_surface(s, c[0], c[1]) {
            None => none += 1,
            Some(k) => {
                let poly: Vec<V3> = pf
                    .sector_polygon(k as usize)
                    .unwrap_or_default()
                    .iter()
                    .map(|&i| pf.vertices[i])
                    .collect();
                if contains_xy(&poly, c[0], c[1]) {
                    ok += 1
                } else {
                    bad += 1
                }
            }
        }
    }
    println!("surface centroid -> sector contains it: ok {ok} bad {bad} none {none}");
    // collision-backed sectors (reached from refs)
    let mut backed = vec![false; pf.sectors.len()];
    let mut stack: Vec<tags::pathfinding::Ref> = pf.refs.clone();
    let mut seen = vec![false; pf.bsp2d_nodes.len()];
    while let Some(r) = stack.pop() {
        match r.target() {
            RefTarget::Sector(k) => backed[k as usize] = true,
            RefTarget::Node(n) => {
                if !seen[n as usize] {
                    seen[n as usize] = true;
                    let nd = &pf.bsp2d_nodes[n as usize];
                    stack.push(nd.positive);
                    stack.push(nd.negative);
                }
            }
            RefTarget::None => {}
        }
    }
    println!(
        "collision-backed sectors {}/{}",
        backed.iter().filter(|b| **b).count(),
        pf.sectors.len()
    );
    for bit in 0..16 {
        let m = 1u16 << bit;
        let (mut b1, mut b0) = (0, 0);
        for (i, s) in pf.sectors.iter().enumerate() {
            if s.flags & m != 0 {
                if backed[i] { b1 += 1 } else { b0 += 1 }
            }
        }
        if b1 + b0 > 0 {
            println!("sector bit {bit}: backed {b1} unbacked {b0}");
        }
    }
    for bit in 0..16 {
        let m = 1u16 << bit;
        let (mut two, mut one) = (0, 0);
        for l in &pf.links {
            if l.flags & m != 0 {
                if l.left_sector != u16::MAX && l.right_sector != u16::MAX {
                    two += 1
                } else {
                    one += 1
                }
            }
        }
        if two + one > 0 {
            println!("link bit {bit}: two-sided {two} one-sided {one}");
        }
    }
    let one_sided_no1 = pf
        .links
        .iter()
        .filter(|l| (l.left_sector == u16::MAX) != (l.right_sector == u16::MAX) && l.flags & 1 == 0)
        .count();
    println!("one-sided links without bit0: {one_sided_no1}");
    let mut ir: BTreeMap<u32, usize> = BTreeMap::new();
    for r in &pf.instance_refs {
        *ir.entry(*r).or_default() += 1;
    }
    println!("instance refs: {:?}", ir.iter().take(6).collect::<Vec<_>>());
    println!("checksum {:08x}", pf.checksum);
    let mut nzs: Vec<(f32, bool)> = surf_poly
        .iter()
        .enumerate()
        .map(|(s, sp)| (sp.1, pf.refs[s] != tags::pathfinding::Ref::NONE))
        .filter(|x| x.0 > 0.6 && x.0 < 0.9)
        .collect();
    nzs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let with: Vec<f32> = nzs.iter().filter(|x| x.1).map(|x| x.0).collect();
    let without: Vec<f32> = nzs.iter().filter(|x| !x.1).map(|x| x.0).collect();
    println!(
        "slope: min nz with ref (0.6..0.9) {:?}, max nz without {:?}",
        with.first(),
        without.last()
    );
    let mut probs: BTreeMap<String, usize> = BTreeMap::new();
    for m in &v {
        *probs
            .entry(m.split(": ").nth(1).unwrap_or(m).to_string())
            .or_default() += 1;
    }
    println!("problem kinds {probs:?}");
    let dang: Vec<_> = pf
        .links
        .iter()
        .enumerate()
        .filter(|(_, l)| l.left_sector == u16::MAX && l.right_sector == u16::MAX)
        .take(2)
        .collect();
    println!("dangling sample {dang:?}");
    let oor: Vec<_> = pf
        .links
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            l.forward_link as usize >= pf.links.len() || l.reverse_link as usize >= pf.links.len()
        })
        .take(3)
        .collect();
    println!("next-out-of-range sample {oor:?}");
    Ok(())
}
