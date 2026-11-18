//! Seam analysis of a structure BSP's drawn cluster geometry: open edges,
//! T-junctions and near-coincident (unwelded) vertices, from your own disc.
//!
//!   bsp_cracks [--disc ISO] <map> [--bsp N] [--tol T] [--list K]
//!
//! Vertices are welded by their exact bits, as the GPU sees them: two
//! triangles share an edge only when both endpoints are bit-identical.
//! Every edge used by exactly one triangle is open. An open edge is then
//! classified:
//!
//! - **T-junction**: another drawn vertex lies on its interior (within
//!   `--tol` world units of the segment, strictly between the ends). The
//!   triangles there do not share the edge's endpoints, so after any
//!   rounding of screen positions a sliver can open along it.
//! - **Unwelded**: an open edge of another triangle has both endpoints
//!   within `--tol` of this one's but not bit-identical.
//! - **Border**: neither (the level's outer edge, or a hole).
//!
//! Each is split by whether the other side is in the same cluster or in
//! another, and whether the two sides use the same material. Prints counts
//! only, and with `--list K` the K longest T-junction edges (positions, for
//! finding them in a capture; printed, never written to a file).
#[path = "../disc.rs"]
#[allow(dead_code)]
mod disc;
#[path = "../level.rs"]
#[allow(dead_code)]
mod level;

use anyhow::{Context, Result, bail};
use disc::Disc;
use level::scenario_bsps;
use std::collections::HashMap;
use std::path::PathBuf;
use tags::structure::StructureRender;

/// A drawn triangle: welded vertex ids, cluster, material.
struct Tri {
    v: [u32; 3],
    cluster: u32,
    material: u16,
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> [f64; 3] {
    [
        f64::from(a[0]) - f64::from(b[0]),
        f64::from(a[1]) - f64::from(b[1]),
        f64::from(a[2]) - f64::from(b[2]),
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Uniform grid over vertex ids.
struct Grid {
    cell: f64,
    cells: HashMap<[i32; 3], Vec<u32>>,
}
impl Grid {
    fn key(&self, p: V3) -> [i32; 3] {
        p.map(|c| (f64::from(c) / self.cell).floor() as i32)
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut disc, mut name, mut bsp_index, mut tol, mut list) =
        (None, None, 0usize, 1e-3f64, 0usize);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--disc" => disc = Some(PathBuf::from(args.next().context("--disc ISO")?)),
            "--bsp" => bsp_index = args.next().context("--bsp N")?.parse()?,
            "--tol" => tol = args.next().context("--tol T")?.parse()?,
            "--list" => list = args.next().context("--list K")?.parse()?,
            _ if name.is_none() && !a.starts_with('-') => name = Some(a),
            _ => bail!("usage: bsp_cracks [--disc ISO] <map> [--bsp N] [--tol T] [--list K]"),
        }
    }
    let name = name.context("no map name")?;
    let disc = Disc::open(&disc::disc_path(disc)?)?;
    let mut map = disc.open_map(&name)?;
    let mut raw = disc.resources(&name)?;
    let bsps = scenario_bsps(&mut map)?;
    let (bsp_id, _) = *bsps.get(bsp_index).context("no such BSP")?;
    let bsp = StructureRender::read(&mut map, bsp_id)?;
    let materials = Some(bsp.material_count as usize);

    let mut positions: Vec<V3> = Vec::new();
    let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
    let mut tris: Vec<Tri> = Vec::new();
    let mut degenerate = 0usize;
    for (ci, c) in bsp.clusters.iter().enumerate() {
        let Some(s) = c.geometry.load(&mut raw, materials)? else {
            continue;
        };
        let p = s.vertices(None)?.positions;
        for part in s.parts.iter().filter(|p| p.is_drawn()) {
            for t in s.part_triangles(part) {
                let v = t.map(|i| {
                    let q = p[usize::from(i)];
                    *ids.entry(q.map(f32::to_bits)).or_insert_with(|| {
                        positions.push(q);
                        positions.len() as u32 - 1
                    })
                });
                if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
                    degenerate += 1;
                    continue;
                }
                tris.push(Tri {
                    v,
                    cluster: ci as u32,
                    material: part.material,
                });
            }
        }
    }

    // Edges by welded endpoints.
    let mut edges: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t.v[k], t.v[(k + 1) % 3]);
            edges
                .entry((a.min(b), a.max(b)))
                .or_default()
                .push(ti as u32);
        }
    }
    let shared_across_clusters = edges
        .values()
        .filter(|ts| ts.len() == 2 && tris[ts[0] as usize].cluster != tris[ts[1] as usize].cluster)
        .count();
    // Shared edges whose two triangles are in different draws: another
    // cluster's section, or another part (material) of the same section.
    // These are the seams two separately transformed draws must close.
    let shared_across_draws = edges
        .values()
        .filter(|ts| {
            let (x, y) = (&tris[ts[0] as usize], ts.get(1).map(|&t| &tris[t as usize]));
            y.is_some_and(|y| x.cluster != y.cluster || x.material != y.material)
        })
        .count();
    let shared_across_materials = edges
        .values()
        .filter(|ts| {
            ts.len() == 2 && tris[ts[0] as usize].material != tris[ts[1] as usize].material
        })
        .count();
    let nonmanifold = edges.values().filter(|ts| ts.len() > 2).count();
    let open: Vec<(u32, u32, u32)> = edges
        .iter()
        .filter(|(_, ts)| ts.len() == 1)
        .map(|(&(a, b), ts)| (a, b, ts[0]))
        .collect();

    // Vertex grid, and the vertices of each triangle (to find the other side).
    let mut grid = Grid {
        cell: 0.5,
        cells: HashMap::new(),
    };
    for (i, &p) in positions.iter().enumerate() {
        let k = grid.key(p);
        grid.cells.entry(k).or_default().push(i as u32);
    }
    let mut vert_tris: Vec<Vec<u32>> = vec![Vec::new(); positions.len()];
    for (ti, t) in tris.iter().enumerate() {
        for &v in &t.v {
            vert_tris[v as usize].push(ti as u32);
        }
    }
    // Open edges by rounded endpoints, for the unwelded test.
    let q = |p: V3| p.map(|c| (f64::from(c) / (tol * 4.0)).round() as i64);
    let mut open_near: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (oi, &(a, _, _)) in open.iter().enumerate() {
        open_near
            .entry(q(positions[a as usize]))
            .or_default()
            .push(oi);
    }

    #[derive(Default)]
    struct Counts {
        same_cluster: usize,
        other_cluster: usize,
        same_material: usize,
        other_material: usize,
    }
    let (mut tj, mut unwelded, mut border) = (Counts::default(), Counts::default(), 0usize);
    let mut tj_edges: Vec<(f64, u32, u32, u32, u32, f64)> = Vec::new();
    let mut tj_dist_hist = [0usize; 8];
    let mut unwelded_gap = 0f64;
    let mut clusters_with_tj = std::collections::BTreeSet::new();
    for &(a, b, ti) in &open {
        let (pa, pb) = (positions[a as usize], positions[b as usize]);
        let d = sub(pb, pa);
        let len2 = dot(d, d);
        let me = &tris[ti as usize];
        // T-junction: a vertex strictly inside the segment.
        let (lo, hi) = (
            grid.key([0, 1, 2].map(|k| pa[k].min(pb[k]) - tol as f32)),
            grid.key([0, 1, 2].map(|k| pa[k].max(pb[k]) + tol as f32)),
        );
        let mut hit: Option<(u32, f64)> = None;
        'cells: for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let Some(vs) = grid.cells.get(&[x, y, z]) else {
                        continue;
                    };
                    for &v in vs {
                        if v == a || v == b {
                            continue;
                        }
                        let w = sub(positions[v as usize], pa);
                        let t = dot(w, d) / len2;
                        if t <= 1e-6 || t >= 1.0 - 1e-6 {
                            continue;
                        }
                        let off = [w[0] - t * d[0], w[1] - t * d[1], w[2] - t * d[2]];
                        let dist = dot(off, off).sqrt();
                        if dist <= tol {
                            hit = Some((v, dist));
                            break 'cells;
                        }
                    }
                }
            }
        }
        if let Some((v, dist)) = hit {
            let other = vert_tris[v as usize]
                .iter()
                .map(|&o| &tris[o as usize])
                .find(|o| o.cluster != me.cluster)
                .or_else(|| vert_tris[v as usize].first().map(|&o| &tris[o as usize]))
                .expect("a drawn vertex has a triangle");
            if other.cluster == me.cluster {
                tj.same_cluster += 1;
            } else {
                tj.other_cluster += 1;
            }
            if other.material == me.material {
                tj.same_material += 1;
            } else {
                tj.other_material += 1;
            }
            clusters_with_tj.insert(me.cluster);
            let bucket = if dist == 0.0 {
                0
            } else {
                ((dist.log10() + 8.0).floor().clamp(1.0, 7.0)) as usize
            };
            tj_dist_hist[bucket] += 1;
            tj_edges.push((len2.sqrt(), a, b, me.cluster, other.cluster, dist));
            continue;
        }
        // Unwelded: another open edge with both ends near ours.
        let mut found = None;
        for oi in open_near.get(&q(pb)).into_iter().flatten() {
            let (c, e, oti) = open[*oi];
            let (pc, pe) = (positions[c as usize], positions[e as usize]);
            let near = |x: V3, y: V3| dot(sub(x, y), sub(x, y)).sqrt() <= tol;
            if near(pc, pb) && near(pe, pa) && (c, e) != (a, b) {
                found = Some((oti, dot(sub(pc, pb), sub(pc, pb)).sqrt()));
                break;
            }
        }
        if found.is_none() {
            for oi in open_near.get(&q(pa)).into_iter().flatten() {
                let (c, e, oti) = open[*oi];
                let (pc, pe) = (positions[c as usize], positions[e as usize]);
                let near = |x: V3, y: V3| dot(sub(x, y), sub(x, y)).sqrt() <= tol;
                if near(pc, pa) && near(pe, pb) && (c, e) != (a, b) {
                    found = Some((oti, dot(sub(pc, pa), sub(pc, pa)).sqrt()));
                    break;
                }
            }
        }
        if let Some((oti, gap)) = found {
            let o = &tris[oti as usize];
            if o.cluster == me.cluster {
                unwelded.same_cluster += 1;
            } else {
                unwelded.other_cluster += 1;
            }
            if o.material == me.material {
                unwelded.same_material += 1;
            } else {
                unwelded.other_material += 1;
            }
            unwelded_gap = unwelded_gap.max(gap);
        } else {
            border += 1;
        }
    }

    println!(
        "{name} bsp {bsp_index}: {} clusters, {} triangles ({degenerate} degenerate skipped), {} welded vertices, {} edges",
        bsp.clusters.len(),
        tris.len(),
        positions.len(),
        edges.len()
    );
    println!(
        "shared edges across clusters (bit-identical ends): {shared_across_clusters}; non-manifold edges: {nonmanifold}"
    );
    println!(
        "shared edges between draws (other cluster or other material): {shared_across_draws}, of which across materials: {shared_across_materials}"
    );
    println!("open edges: {} (tolerance {tol} world units)", open.len());
    println!(
        "  T-junctions: {} (other side same cluster {}, other cluster {}; same material {}, other {}); in {} clusters",
        tj.same_cluster + tj.other_cluster,
        tj.same_cluster,
        tj.other_cluster,
        tj.same_material,
        tj.other_material,
        clusters_with_tj.len()
    );
    println!(
        "    vertex distance from the edge: exactly 0: {}, <1e-7: {}, 1e-7..1e-6: {}, 1e-6..1e-5: {}, 1e-5..1e-4: {}, 1e-4..1e-3: {}, >=1e-3: {}",
        tj_dist_hist[0],
        tj_dist_hist[1],
        tj_dist_hist[2],
        tj_dist_hist[3],
        tj_dist_hist[4],
        tj_dist_hist[5],
        tj_dist_hist[6] + tj_dist_hist[7]
    );
    println!(
        "  unwelded (both ends within tolerance, not bit-identical): {} (same cluster {}, other {}; same material {}, other {}); largest gap {unwelded_gap:.2e}",
        unwelded.same_cluster + unwelded.other_cluster,
        unwelded.same_cluster,
        unwelded.other_cluster,
        unwelded.same_material,
        unwelded.other_material
    );
    println!("  border (no partner): {border}");
    if list > 0 {
        tj_edges.sort_by(|x, y| y.0.total_cmp(&x.0));
        for &(len, a, b, c, o, dist) in tj_edges.iter().take(list) {
            println!(
                "  T edge len {len:.3} cluster {c} (vertex from cluster {o}, off by {dist:.1e}): {:?} -> {:?}",
                positions[a as usize], positions[b as usize]
            );
        }
    }
    Ok(())
}
