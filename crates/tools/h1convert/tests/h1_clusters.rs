//! Halo 1's clusters as the structure keeps them, from the owner's Halo 1
//! disc (`$H1_ISO`). Only counts and ranges are printed.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;

fn levels() -> Vec<(String, H1Map)> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names = h1convert::convert::MAPS
        .iter()
        .map(|m| m.0)
        .chain(h1map::disc::CAMPAIGN_LEVELS.iter().copied());
    let mut out = Vec::new();
    for n in names {
        let Some(e) = disc.map(n).cloned() else {
            continue;
        };
        let m = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        out.push((n.to_owned(), m));
    }
    out
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

/// The fields `h1convert::structure::sound_zones` reads: each cluster's
/// background sound (`+0x04`) and sound environment (`+0x06`) is -1 or
/// indexes the structure's palettes (`+0x1fc`, `+0x208`), and the
/// structure has a leaf (`+0xe0`, cluster at `+0x08`) per collision leaf,
/// each naming a real cluster.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn clusters_name_their_sounds_and_leaves_their_cluster() {
    let mut bsps = 0;
    for (name, h1) in levels() {
        for bsp in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&bsp).unwrap();
            let clusters = s.space.block_at(s.root, 0x134, 0x68).unwrap();
            let leaves = s.space.block_at(s.root, 0xe0, 0x10).unwrap();
            let bg = s.space.block_at(s.root, 0x1fc, 0x74).unwrap().len() as i16;
            let env = s.space.block_at(s.root, 0x208, 0x50).unwrap().len() as i16;
            let coll = s.collision_bsps().unwrap()[0].leaves.len();
            let mut zones = std::collections::BTreeSet::new();
            for (k, c) in clusters.iter().enumerate() {
                let (b, e) = (i16_at(c, 4), i16_at(c, 6));
                assert!(
                    (-1..bg).contains(&b),
                    "{name} {}: cluster {k} sound {b} of {bg}",
                    bsp.name
                );
                assert!(
                    (-1..env).contains(&e),
                    "{name} {}: cluster {k} environment {e} of {env}",
                    bsp.name
                );
                zones.insert((b, e));
            }
            assert_eq!(leaves.len(), coll, "{name} {}", bsp.name);
            for (k, l) in leaves.iter().enumerate() {
                let c = i16_at(l, 8);
                assert!(
                    c >= 0 && (c as usize) < clusters.len(),
                    "{name} {}: leaf {k} cluster {c}",
                    bsp.name
                );
            }
            println!(
                "{name} {}: {} clusters, {} sound zones, {} leaves",
                bsp.name,
                clusters.len(),
                zones.len(),
                leaves.len()
            );
            bsps += 1;
        }
    }
    assert!(bsps > 0);
}

/// Halo 1's portals as `h1convert::clusters::portals` converts them: every
/// portal joins two clusters, lies in its collision plane (each polygon
/// vertex within a centimetre of it) and is on both clusters' lists, and
/// every cluster with geometry is reached from every other by crossing
/// portals, so the flood from any camera cluster can reach the whole level
/// (unless the structure has no portals, as Chiron TL-34's sealed rooms
/// don't).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn portals_join_every_cluster_in_their_planes() {
    let mut bsps = 0;
    for (name, h1) in levels() {
        for bsp in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&bsp).unwrap();
            let col = &s.collision_bsps().unwrap()[0];
            let n = h1convert::clusters::cluster_count(&s).unwrap();
            let v = h1convert::clusters::portals(&s, n, col.planes.len()).unwrap();
            assert_eq!(v.clusters.len(), n, "{name} {}", bsp.name);
            let mut joined: Vec<Vec<usize>> = vec![Vec::new(); n];
            for (k, p) in v.portals.iter().enumerate() {
                let [a, b] = p.clusters.map(usize::from);
                assert!(
                    v.clusters[a].contains(&(k as u16)) && v.clusters[b].contains(&(k as u16)),
                    "{name} {}: portal {k} is not on both its clusters' lists",
                    bsp.name
                );
                let pl = col.planes[p.plane as usize];
                for q in &p.vertices {
                    let d = pl[0] * q[0] + pl[1] * q[1] + pl[2] * q[2] - pl[3];
                    assert!(
                        d.abs() < 0.01,
                        "{name} {}: portal {k} is {d} off its plane",
                        bsp.name
                    );
                }
                joined[a].push(b);
                joined[b].push(a);
            }
            let surfaces = s.render_surfaces().unwrap().len();
            let owner = h1convert::clusters::surface_clusters(&s, surfaces).unwrap();
            let mut drawn = vec![false; n];
            for c in owner.iter().flatten() {
                drawn[usize::from(*c)] = true;
            }
            let Some(start) = drawn.iter().position(|&d| d) else {
                continue;
            };
            let mut seen = vec![false; n];
            let mut stack = vec![start];
            seen[start] = true;
            while let Some(c) = stack.pop() {
                for &o in &joined[c] {
                    if !seen[o] {
                        seen[o] = true;
                        stack.push(o);
                    }
                }
            }
            let cut_off: Vec<usize> = (0..n).filter(|&c| drawn[c] && !seen[c]).collect();
            // A structure with no portals at all (Chiron TL-34's rooms) is
            // drawn a cluster at a time in Halo 1 too.
            assert!(
                cut_off.is_empty() || v.portals.is_empty(),
                "{name} {}: clusters {cut_off:?} can't be reached",
                bsp.name
            );
            println!(
                "{name} {}: {n} clusters, {} portals, {} sealed off, {} surfaces in no cluster",
                bsp.name,
                v.portals.len(),
                cut_off.len(),
                owner.iter().filter(|o| o.is_empty()).count()
            );
            bsps += 1;
        }
    }
    assert!(bsps > 0);
}

/// Every level's render geometry laid out by cluster
/// (`h1convert::clusters::lay_out`): one section per Halo 1 cluster, each
/// lit one on a page of its own, and every lit vertex sampling the same
/// light there as on its Halo 1 page, halved onto Halo 2's scale
/// (filtered bilinearly, clamped at the edge, from both bitmaps decoded;
/// within half a 5-bit step and one step of 255). Prints each structure's page texels against Halo 1's.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_cluster_samples_its_own_lightmap_page_as_halo1_did() {
    use h1_assets::bitmaps::BitmapExt;
    use h1map::bitmap::Bitmap;
    let mut bsps = 0;
    for (name, h1) in levels() {
        for bsp in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&bsp).unwrap();
            let col = &s.collision_bsps().unwrap()[0];
            let before = h1convert::structure::meshes(&s, |_| Ok(String::new())).unwrap();
            let mut after = before.clone();
            let material = vec![0u16; after.len()];
            let laid =
                h1convert::clusters::lay_out(&h1, &s, &mut after, &material, col.planes.len())
                    .unwrap();
            let n = h1convert::clusters::cluster_count(&s).unwrap();
            assert_eq!(laid.sections.len(), n, "{name} {}", bsp.name);
            for (c, sec) in laid.sections.iter().enumerate() {
                if let Some(p) = sec.page {
                    assert!(usize::from(p) < laid.pages.len(), "{name} cluster {c}");
                }
            }
            let Some((_, atlas, converted)) =
                h1convert::clusters::lightmap_bitmap(&h1, &s, &laid.pages).unwrap()
            else {
                continue;
            };
            let tag = h1.tag(s.lightmaps_bitmap().id).unwrap().clone();
            let source = Bitmap::read(&h1, &tag).unwrap();
            let decode_source: Vec<_> = (0..source.images.len())
                .map(|i| source.decode(&h1, i, 0).unwrap())
                .collect();
            let decode_atlas: Vec<_> = atlas
                .images
                .iter()
                .zip(&converted.raw)
                .map(|(img, (bytes, _))| {
                    h1_assets::bitmaps::decode::stack(&h1_assets::bitmaps::decode_image(img, bytes).unwrap(), 0)
                        .unwrap()
                })
                .collect();
            // Bilinear, clamped to the edge, as the page is sampled.
            let texel = |im: &h1_assets::bitmaps::Rgba8Image, uv: [f32; 2]| -> [f32; 4] {
                let (w, h) = (im.width as i64, im.height as i64);
                let (x, y) = (uv[0] * w as f32 - 0.5, uv[1] * h as f32 - 0.5);
                let (x0, y0) = (x.floor(), y.floor());
                let (fx, fy) = (x - x0, y - y0);
                let at = |xi: i64, yi: i64, ch: usize| {
                    let (xi, yi) = (xi.clamp(0, w - 1), yi.clamp(0, h - 1));
                    f32::from(im.rgba[4 * (yi * w + xi) as usize + ch])
                };
                let (x0, y0) = (x0 as i64, y0 as i64);
                std::array::from_fn(|ch| {
                    let top = at(x0, y0, ch) * (1.0 - fx) + at(x0 + 1, y0, ch) * fx;
                    let bottom = at(x0, y0 + 1, ch) * (1.0 - fx) + at(x0 + 1, y0 + 1, ch) * fx;
                    top * (1.0 - fy) + bottom * fy
                })
            };
            let (mut samples, mut differ) = (0usize, 0usize);
            for (b, a) in before.iter().zip(&after) {
                let (Some(p), Some(q)) = (b.page, a.page) else {
                    continue;
                };
                if usize::from(p) >= decode_source.len() {
                    continue;
                }
                for &i in b.triangles.iter().flatten() {
                    let i = i as usize;
                    samples += 1;
                    let want = texel(&decode_source[usize::from(p)], b.vertices[i].lightmap);
                    let got = texel(&decode_atlas[usize::from(q)], a.vertices[i].lightmap);
                    // Half Halo 1's light (`h1_assets::bitmaps::atlas::to_halo2_light`):
                    // within half a 5-bit step, and one 8-bit step of filtering.
                    differ += usize::from(
                        want.iter().zip(&got).take(3).any(|(a, b)| (a / 2.0 - b).abs() > 255.0 / 62.0 + 1.0),
                    );
                }
            }
            assert!(
                differ == 0,
                "{name} {}: {differ} of {samples} lightmap samples differ",
                bsp.name
            );
            let texels = |imgs: &[h1map::bitmap::Image]| -> u64 {
                imgs.iter()
                    .map(|i| u64::from(i.width) * u64::from(i.height))
                    .sum()
            };
            println!(
                "{name} {}: {n} clusters, {} pages of {} texels from {} of {} ({differ} of {samples} samples differ)",
                bsp.name,
                atlas.images.len(),
                texels(&atlas.images),
                source.images.len(),
                texels(&source.images),
            );
            bsps += 1;
        }
    }
    assert!(bsps > 0);
}
