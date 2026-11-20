//! The engine's portal flood (`h1convert::flood`, the model of
//! `0x00173130`) run offline over converted Halo 1 structures, from the
//! owner's Halo 1 disc (`$H1_ISO`). Only counts are printed.
//!
//! H1_ISO=disc/halo1.iso cargo test --release -p h1convert --test portal_flood -- --ignored --nocapture

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::flood::{self, NODES, View};
use h1map::H1Map;

fn level(name: &str) -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name).cloned().expect("map on the disc");
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

struct Level {
    portals: Vec<flood::Portal>,
    lists: Vec<Vec<u16>>,
    /// Per cluster, its render triangles' corners.
    geometry: Vec<(Vec<[f32; 3]>, Vec<[u32; 3]>)>,
    leaf_cluster: Vec<u16>,
    col: h1map::bsp::CollisionBsp,
    spawns: Vec<[f32; 3]>,
}

fn load(h1: &H1Map) -> Level {
    let bsp = &h1.structure_bsps().unwrap()[0];
    let s = h1.structure_bsp(bsp).unwrap();
    let col = s.collision_bsps().unwrap().remove(0);
    let n = h1convert::clusters::cluster_count(&s).unwrap();
    let v = h1convert::clusters::portals(&s, n, col.planes.len()).unwrap();
    let portals = v
        .portals
        .iter()
        .map(|p| flood::Portal {
            clusters: p.clusters,
            plane: col.planes[p.plane as usize],
            vertices: p.vertices.clone(),
        })
        .collect();
    let meshes = h1convert::structure::meshes(&s, |_| Ok(String::new())).unwrap();
    let mut geometry = vec![(Vec::new(), Vec::new()); n];
    for m in &meshes {
        let (vs, ts): &mut (Vec<[f32; 3]>, Vec<[u32; 3]>) = &mut geometry[usize::from(m.cluster)];
        let base = vs.len() as u32;
        vs.extend(m.vertices.iter().map(|v| v.position));
        ts.extend(m.triangles.iter().map(|t| t.map(|i| i + base)));
    }
    let (_, leaf_cluster) =
        h1convert::clusters::sound_zones(&s, col.leaves.len(), |z| z).unwrap();
    let spawns = h1
        .multiplayer()
        .map(|m| {
            m.starting_locations
                .iter()
                .map(|l| [l.position[0], l.position[1], l.position[2] + 0.62])
                .collect()
        })
        .unwrap_or_default();
    Level {
        portals,
        lists: v.clusters,
        geometry,
        leaf_cluster,
        col,
        spawns,
    }
}

impl Level {
    fn cluster_at(&self, p: [f32; 3]) -> Option<usize> {
        let leaf = self.col.leaf_at(p)?;
        Some(usize::from(self.leaf_cluster[leaf as usize]))
    }

    /// Whether any of cluster `c`'s triangles is seen from `view`: its
    /// centre in the frustum and the segment to it (stopping 5 cm short)
    /// clear of solid. Up to `samples` triangles are tried, spread
    /// through the cluster.
    fn seen(&self, view: &View, c: usize, samples: usize) -> bool {
        self.seen_at(view, c, samples).is_some()
    }

    /// The first sample point of [`Self::seen`].
    fn seen_at(&self, view: &View, c: usize, samples: usize) -> Option<[f32; 3]> {
        let (v, t) = &self.geometry[c];
        let planes = view.planes();
        let inside = |p: [f32; 3]| {
            planes
                .iter()
                .all(|(n, d)| n[0] * p[0] + n[1] * p[1] + n[2] * p[2] >= *d)
        };
        let step = (t.len() / samples.max(1)).max(1);
        t.iter().step_by(step).find_map(|tri| {
            let p = tri.iter().fold([0.0f32; 3], |a, &i| {
                let q = v[i as usize];
                [a[0] + q[0] / 3.0, a[1] + q[1] / 3.0, a[2] + q[2] / 3.0]
            });
            if !inside(p) {
                return None;
            }
            let e = view.position;
            let d = [p[0] - e[0], p[1] - e[1], p[2] - e[2]];
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if len < 0.1 {
                return Some(p);
            }
            let k = (len - 0.05) / len;
            let end = [e[0] + d[0] * k, e[1] + d[1] * k, e[2] + d[2] * k];
            h1convert::query::first_hit(&self.col, e, end).is_none().then_some(end)
        })
    }

    /// Walks the sight line `a`→`b` and, at each change of cluster, says
    /// which portal joining the two the crossing goes through, or how far
    /// it passes from the nearest.
    fn crossings(&self, a: [f32; 3], b: [f32; 3]) -> Vec<String> {
        let mut out = Vec::new();
        let steps = 2000;
        let at = |t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let mut prev = self.cluster_at(a);
        for i in 1..=steps {
            let p = at(i as f32 / steps as f32);
            let c = self.cluster_at(p);
            if c != prev {
                let (Some(x), Some(y)) = (prev, c) else {
                    out.push(format!("{prev:?}->{c:?} at {p:?}"));
                    prev = c;
                    continue;
                };
                let mut best = (f32::MAX, usize::MAX, 0.0f32);
                for (k, q) in self.portals.iter().enumerate() {
                    let pair = q.clusters.map(usize::from);
                    if pair != [x, y] && pair != [y, x] {
                        continue;
                    }
                    let (d, plane) = poly_distance(q, p);
                    if d < best.0 {
                        best = (d, k, plane);
                    }
                }
                out.push(format!(
                    "{x}->{y} at {:?}: portal {} {} off its polygon ({} off its plane)",
                    p.map(|v| (v * 10.0).round() / 10.0),
                    best.1 as isize,
                    best.0,
                    best.2
                ));
                prev = c;
            }
        }
        out
    }
}

/// How far `p` lies from portal `q`'s polygon within its plane (0 inside),
/// and how far off its plane.
fn poly_distance(q: &flood::Portal, p: [f32; 3]) -> (f32, f32) {
    let n = [q.plane[0], q.plane[1], q.plane[2]];
    let off = n[0] * p[0] + n[1] * p[1] + n[2] * p[2] - q.plane[3];
    let p = [p[0] - n[0] * off, p[1] - n[1] * off, p[2] - n[2] * off];
    let v = &q.vertices;
    let mut outside = 0.0f32;
    let mut sign = 0.0f32;
    for i in 0..v.len() {
        let (a, b) = (v[i], v[(i + 1) % v.len()]);
        let e = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let c = [e[1] * w[2] - e[2] * w[1], e[2] * w[0] - e[0] * w[2], e[0] * w[1] - e[1] * w[0]];
        let s = c[0] * n[0] + c[1] * n[1] + c[2] * n[2];
        let len = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt().max(1e-6);
        if sign == 0.0 && s.abs() > 1e-6 {
            sign = s.signum();
        }
        if s * sign < 0.0 {
            outside = outside.max(-s * sign / len);
        }
    }
    (outside, off)
}

/// Which side of each portal's plane its clusters are on, by the
/// collision BSP: the cluster of the leaf just behind and just in front of
/// the portal's centre. The flood crosses from `+0x00` when the camera is
/// behind the plane, so `+0x00` must be the cluster behind.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn bloodgulch_portals_name_the_cluster_behind_first() {
    let h1 = level("bloodgulch");
    let l = load(&h1);
    let (mut agree, mut swapped, mut unknown) = (0, 0, 0);
    for (k, p) in l.portals.iter().enumerate() {
        let n = [p.plane[0], p.plane[1], p.plane[2]];
        let c = p.vertices.iter().fold([0.0f32; 3], |a, v| {
            [a[0] + v[0], a[1] + v[1], a[2] + v[2]]
        });
        let c = c.map(|x| x / p.vertices.len() as f32);
        let at = |s: f32| {
            l.cluster_at([c[0] + n[0] * s, c[1] + n[1] * s, c[2] + n[2] * s])
        };
        let (back, front) = (at(-0.05), at(0.05));
        let want = p.clusters.map(|c| Some(usize::from(c)));
        if [back, front] == want {
            agree += 1;
        } else if [front, back] == want {
            swapped += 1;
            println!("portal {k}: {:?} but the BSP says back {back:?} front {front:?}", p.clusters);
        } else {
            unknown += 1;
            println!("portal {k}: {:?}, the BSP says back {back:?} front {front:?}", p.clusters);
        }
    }
    println!("bloodgulch: {} portals: {agree} agree, {swapped} swapped, {unknown} unknown", l.portals.len());
    assert_eq!(swapped, 0);
}

/// From every spawn, turning through 360 degrees: the clusters with
/// geometry in the frustum, those the flood reaches without a node limit,
/// and those it reaches with the engine's [`NODES`].
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn bloodgulch_flood_from_the_spawns() {
    let h1 = level("bloodgulch");
    let l = load(&h1);
    let fov = 78f32.to_radians();
    let n = l.lists.len();
    println!("bloodgulch: {n} clusters, {} portals, {} spawns", l.portals.len(), l.spawns.len());
    let (mut views, mut short, mut lost_by_limit, mut lost_by_portals, mut lost_by_facing) = (0, 0, 0, 0, 0);
    let mut max_nodes = 0;
    for (si, &spawn) in l.spawns.iter().enumerate() {
        let Some(start) = l.cluster_at(spawn) else {
            println!("spawn {si}: in solid");
            continue;
        };
        for step in 0..36 {
            let yaw = (step as f32 * 10.0).to_radians();
            let view = View::level(spawn, yaw, fov);
            let truth: Vec<bool> = (0..n).map(|c| l.seen(&view, c, 2000)).collect();
            let free = flood::flood(&view, &l.portals, &l.lists, start, 20_000);
            let capped = flood::flood(&view, &l.portals, &l.lists, start, NODES);
            let unclipped = flood::flood_with(&view, &l.portals, &l.lists, start, 20_000, false);
            max_nodes = max_nodes.max(free.nodes);
            views += 1;
            let missing: Vec<usize> = (0..n).filter(|&c| truth[c] && !capped.clusters[c]).collect();
            let by_portals: Vec<usize> = (0..n).filter(|&c| truth[c] && !free.clusters[c]).collect();
            let by_facing: Vec<usize> = (0..n).filter(|&c| truth[c] && !unclipped.clusters[c]).collect();
            if !missing.is_empty() {
                short += 1;
                lost_by_facing += by_facing.len();
                lost_by_limit += missing.len() - by_portals.len();
                lost_by_portals += by_portals.len();
                if short <= 12 {
                    for &c in &missing {
                        if let Some(p) = l.seen_at(&view, c, 2000) {
                            println!("  spawn {si} yaw {} cluster {c}: {:?}", step * 10, l.crossings(spawn, p));
                        }
                    }
                }
                if step % 3 == 0 {
                    println!(
                        "spawn {si} yaw {:3}: start {start}, nodes {} (free {}), dropped {}, missing {missing:?}, of which the portals miss {by_portals:?}, not even unclipped {by_facing:?}",
                        step * 10,
                        capped.nodes,
                        free.nodes,
                        capped.dropped
                    );
                }
            }
        }
    }
    println!(
        "{views} views, {short} miss a cluster seen: {lost_by_limit} lost to the node limit, {lost_by_portals} the portals never reach ({lost_by_facing} not even without the screen test); most nodes a free flood makes: {max_nodes}"
    );
    assert_eq!(lost_by_limit, 0);
    assert!(max_nodes < NODES);
}

/// Halo 1's cluster `+0x00` (i16): the sky the cluster shows, an index
/// into the scenario's skies or -1 for none. Prints, per level and
/// structure, how many clusters name each sky.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_clusters_name_their_sky() {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names = h1convert::convert::MAPS
        .iter()
        .map(|m| m.0)
        .chain(h1map::disc::CAMPAIGN_LEVELS.iter().copied());
    for n in names {
        let Some(e) = disc.map(n).cloned() else {
            continue;
        };
        let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let skies: Vec<String> = h1
            .multiplayer()
            .unwrap()
            .skies
            .iter()
            .map(|r| h1.tag(r.id).map(|t| t.name.clone()).unwrap_or_default())
            .collect();
        for bsp in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&bsp).unwrap();
            let clusters = s.space.block_at(s.root, 0x134, 0x68).unwrap();
            let mut count = std::collections::BTreeMap::new();
            for c in &clusters {
                let sky = i16::from_le_bytes([c[0], c[1]]);
                assert!(
                    (-1..skies.len() as i16).contains(&sky),
                    "{n} {}: sky {sky} of {}",
                    bsp.name,
                    skies.len()
                );
                *count.entry(sky).or_insert(0) += 1;
            }
            println!("{n} {}: skies {skies:?}; clusters per sky {count:?}", bsp.name);
        }
    }
}

/// The sky each converted cluster shows (`clusters::cluster_skies`, cluster
/// `+0x6e`) on a10: the cryo bay (`a10a`) has no cluster with a Halo 1 sky
/// and no portal to one, so none shows a sky (-1, `0xff`), as Halo 1 draws
/// none there; sky 0 there drew as the grey slab over the start of the
/// level. `a10b` has clusters with a sky (its windows on space), so every
/// cluster those reach keeps one. Every cluster of every a10 structure
/// shows a sky its Halo 1 cluster names or reaches through portals, or none.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn a10_clusters_no_sky_reaches_show_none() {
    let h1 = level("a10");
    let mut seen = Vec::new();
    for bsp in h1.structure_bsps().unwrap() {
        let s = h1.structure_bsp(&bsp).unwrap();
        let col = s.collision_bsps().unwrap().remove(0);
        let n = h1convert::clusters::cluster_count(&s).unwrap();
        let v = h1convert::clusters::portals(&s, n, col.planes.len()).unwrap();
        let own: Vec<i16> = s
            .space
            .block_at(s.root, 0x134, 0x68)
            .unwrap()
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        println!("a10 {}: Halo 1 {own:?} -> converted {:?}", bsp.name, v.skies);
        assert_eq!(v.skies.len(), n);
        for (c, &sky) in v.skies.iter().enumerate() {
            if own[c] >= 0 {
                assert_eq!(i16::from(sky), own[c], "{} cluster {c} keeps its sky", bsp.name);
            }
        }
        if own.iter().all(|&o| o < 0) {
            assert!(
                v.skies.iter().all(|&k| k == -1),
                "{}: no cluster has a sky, so none shows one",
                bsp.name
            );
        }
        seen.push((bsp.name.clone(), v.skies.clone()));
    }
    let (_, a10a) = seen.iter().find(|(n, _)| n.ends_with("a10a")).expect("a10a");
    assert!(a10a.iter().all(|&s| s == -1), "the cryo bay shows no sky");
    let (_, a10b) = seen.iter().find(|(n, _)| n.ends_with("a10b")).expect("a10b");
    assert!(a10b.iter().any(|&s| s >= 0), "a10b's windows show space");
}

/// What Blood Gulch's and a10's skies convert to: the model scale, each
/// material's Halo 1 shader and the recipe it draws with.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn bloodgulch_and_a10_skies() {
    for name in ["bloodgulch", "a10"] {
        let h1 = level(name);
        let reach = h1sky::convert::level_reach(&h1).unwrap();
        for r in h1.multiplayer().unwrap().skies {
            let tag = h1.tag(r.id).unwrap();
            let sky = h1convert::sky::Sky::from_map(&h1, tag).unwrap();
            println!(
                "{name} {}: reach {reach}, model {:?}, scale {}, animation {}, lights {}",
                sky.name,
                sky.model.as_ref().map(|m| &m.0),
                sky.model_scale,
                !sky.sky.animation.id.is_none(),
                sky.sky.lights.len()
            );
            for (k, s) in sky.shaders.iter().enumerate() {
                match s {
                    Some(s) => println!(
                        "  material {k}: {} chicago {} additive {} maps {:?} -> {:?}",
                        s.name,
                        s.chicago,
                        s.additive,
                        s.maps.iter().map(|m| m.scale).collect::<Vec<_>>(),
                        h1convert::sky_shaders::recipe(s)
                    ),
                    None => println!("  material {k}: no shader read (the base sky's)"),
                }
            }
        }
    }
}

/// Every cluster whose subclusters list each render surface (Halo 1
/// cluster `+0x34`, a surface index block at `+0x18`).
fn listing_clusters(s: &h1map::bsp::StructureBsp, surfaces: usize) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); surfaces];
    for (c, cluster) in s.space.block_at(s.root, 0x134, 0x68).unwrap().iter().enumerate() {
        for sub in s.space.block_at(cluster, 0x34, 0x24).unwrap() {
            for ix in s.space.block_at(sub, 0x18, 4).unwrap() {
                let i = u32::from_le_bytes(ix[..4].try_into().unwrap()) as usize;
                if let Some(o) = out.get_mut(i) {
                    if !o.contains(&c) {
                        o.push(c);
                    }
                }
            }
        }
    }
    out
}

/// Halo 1 lists a render surface that crosses a portal in the
/// subclusters of every cluster it is in. Drawing it with the first only
/// loses it whenever the camera sees it from another of its clusters
/// without seeing the first. From every spawn over 360 degrees, the
/// surfaces seen (centre in the frustum, the sight line clear of solid)
/// whose clusters the engine's flood doesn't reach: with the first
/// cluster only, and with every listing cluster.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn bloodgulch_surfaces_in_two_clusters() {
    let h1 = level("bloodgulch");
    let l = load(&h1);
    let bsp = &h1.structure_bsps().unwrap()[0];
    let s = h1.structure_bsp(bsp).unwrap();
    let surfaces = s.render_surfaces().unwrap();
    let lists = listing_clusters(&s, surfaces.len());
    let multi = lists.iter().filter(|l| l.len() > 1).count();
    println!(
        "bloodgulch: {} surfaces, {multi} listed by more than one cluster, {} by none",
        surfaces.len(),
        lists.iter().filter(|l| l.is_empty()).count()
    );
    // Each surface's centre.
    let mut centre = vec![None; surfaces.len()];
    for lm in s.lightmaps().unwrap() {
        for m in lm.materials {
            for k in 0..m.surface_count as usize {
                let i = m.first_surface as usize + k;
                let Some(t) = surfaces.get(i) else { continue };
                let p = t.map(|v| m.vertices[usize::from(v)].position);
                centre[i] = Some(std::array::from_fn::<f32, 3, _>(|a| (p[0][a] + p[1][a] + p[2][a]) / 3.0));
            }
        }
    }
    let fov = 78f32.to_radians();
    let (mut seen, mut lost_first, mut lost_any) = (0usize, 0usize, 0usize);
    let (mut views, mut views_lost_first, mut views_lost_any) = (0, 0, 0);
    for &spawn in &l.spawns {
        let Some(start) = l.cluster_at(spawn) else { continue };
        for step in 0..36 {
            let view = View::level(spawn, (step as f32 * 10.0).to_radians(), fov);
            let planes = view.planes();
            let r = flood::flood(&view, &l.portals, &l.lists, start, NODES);
            let (mut lf, mut la) = (0, 0);
            for (i, c) in centre.iter().enumerate().step_by(3) {
                let Some(p) = *c else { continue };
                if lists[i].is_empty()
                    || !planes.iter().all(|(n, d)| n[0] * p[0] + n[1] * p[1] + n[2] * p[2] >= *d)
                {
                    continue;
                }
                let e = view.position;
                let dv = [p[0] - e[0], p[1] - e[1], p[2] - e[2]];
                let len = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
                if len > 0.1 {
                    let k = (len - 0.05) / len;
                    let end = [e[0] + dv[0] * k, e[1] + dv[1] * k, e[2] + dv[2] * k];
                    if h1convert::query::first_hit(&l.col, e, end).is_some() {
                        continue;
                    }
                }
                seen += 1;
                if !r.clusters[lists[i][0]] {
                    lf += 1;
                }
                if !lists[i].iter().any(|&c| r.clusters[c]) {
                    la += 1;
                }
            }
            views += 1;
            lost_first += lf;
            lost_any += la;
            views_lost_first += usize::from(lf > 0);
            views_lost_any += usize::from(la > 0);
        }
    }
    println!(
        "{views} views, {seen} surfaces seen: drawn with the first cluster, {lost_first} lost in {views_lost_first} views; with every listing cluster, {lost_any} lost in {views_lost_any} views"
    );
    assert!(lost_first > 0, "the first cluster alone loses seen surfaces");
    assert_eq!(lost_any, 0, "drawn with every cluster that lists it, nothing seen is lost");
    // The conversion draws each surface with every cluster listing it.
    assert_eq!(h1convert::clusters::surface_clusters(&s, surfaces.len()).unwrap(),
        lists.iter().map(|l| l.iter().map(|&c| c as u16).collect::<Vec<_>>()).collect::<Vec<_>>());
}

/// Blood Gulch's render geometry by Halo 1 shader: triangles and the
/// height range, to see what of the level stands where the sky shows.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn bloodgulch_render_shaders() {
    let h1 = level("bloodgulch");
    let bsp = &h1.structure_bsps().unwrap()[0];
    let s = h1.structure_bsp(bsp).unwrap();
    let meshes = h1convert::structure::meshes(&s, |r| {
        Ok(h1.tag(r.id).map(|t| format!("{} {}", t.group, t.name)).unwrap_or_else(|_| "none".into()))
    })
    .unwrap();
    let mut by: std::collections::BTreeMap<String, (usize, f32, f32)> = Default::default();
    for m in &meshes {
        let e = by.entry(m.shader.clone()).or_insert((0, f32::MAX, f32::MIN));
        e.0 += m.triangles.len();
        for t in &m.triangles {
            for &i in t {
                let z = m.vertices[i as usize].position[2];
                e.1 = e.1.min(z);
                e.2 = e.2.max(z);
            }
        }
    }
    for (k, (n, lo, hi)) in by {
        println!("{k}: {n} triangles, z {lo}..{hi}");
    }
    println!("world bounds {:?}", s.world_bounds());
}
