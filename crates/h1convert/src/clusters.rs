//! Halo 1's clusters and portals, kept.
//!
//! Halo 2 draws a structure by flooding portals from the camera's cluster
//! (E `0x00173130`, `0x00172840`): a cluster is drawn only when a chain of
//! portals reaches it through the view. A converted structure with no
//! portals draws only the cluster the camera's collision leaf names, so
//! the level needs Halo 1's own clusters and portals, which have the same
//! shape in both games (`tests/h1_clusters.rs`):
//!
//! - every rendered surface belongs to the Halo 1 clusters whose
//!   subclusters (cluster `+0x34`, `0x24` each, a surface index block at
//!   `+0x18`) list it, and becomes part of each one's section
//!   ([`surface_clusters`], [`pack_clusters`]);
//! - Halo 1's portals (`sbsp +0x154`, `0x40` each) become Halo 2's
//!   (`sbsp +0x5c`, `0x24` each) field for field, and each cluster's
//!   portal list (`+0x5c`, i16) its `+0x8c` list ([`portals`]);
//! - every collision leaf names its Halo 1 cluster ([`sound_zones`]);
//! - a cluster's lightmap is one page of its own, cut from the Halo 1
//!   pages its surfaces sample ([`lightmap_pages`], [`h1bitmaps::atlas`]).
//!
//! Layouts and evidence: docs/halo1/multiplayer.md, "Clusters and
//! portals".

use std::collections::BTreeMap;

use h1bitmaps::atlas::{ClusterPage, Piece};
use h1map::H1Map;
use h1map::bitmap::Bitmap;
use h1map::bsp::StructureBsp;
use h1models::Converted;

use crate::structure::{Mesh, SECTION_MAX, Section, Vertex};
use crate::{Error, Result};

/// Halo 1 `sbsp` clusters (block field, element size).
pub const H1_CLUSTERS: usize = 0x134;
pub const H1_CLUSTER: usize = 0x68;
/// A Halo 1 cluster's subclusters (field, element size) and a
/// subcluster's surface indices (field, `u32` each).
pub const H1_SUBCLUSTERS: usize = 0x34;
pub const H1_SUBCLUSTER: usize = 0x24;
pub const H1_SUBCLUSTER_SURFACES: usize = 0x18;
/// A Halo 1 cluster's portal indices (field, i16 each).
pub const H1_CLUSTER_PORTALS: usize = 0x5c;
/// Halo 1 `sbsp` portals (block field, element size) and a portal's
/// vertex block (field, 12 bytes each).
pub const H1_PORTALS: usize = 0x154;
pub const H1_PORTAL: usize = 0x40;
pub const H1_PORTAL_VERTICES: usize = 0x34;
/// Halo 1 `sbsp` leaves (block field, element size), the cluster an i16
/// at `+0x08`.
pub const H1_LEAVES: usize = 0xe0;
pub const H1_LEAF: usize = 0x10;

/// Halo 2 portal element size (`sbsp +0x5c`; E `0x00173572`: index * 0x24).
pub const PORTAL: usize = 0x24;
/// Most portals a structure has: the open-portal mask is 16 words
/// (E `0x00210df7`).
pub const MAX_PORTALS: usize = 512;

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4"))
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().expect("4"))
}

/// Halo 1's cluster count, at least one (a structure with none is drawn
/// as one cluster).
pub fn cluster_count(bsp: &StructureBsp) -> Result<usize> {
    Ok(bsp
        .space
        .block_at(bsp.root, H1_CLUSTERS, H1_CLUSTER)?
        .len()
        .max(1))
}

/// The Halo 1 clusters of each of `surfaces` render surfaces: every
/// cluster whose subclusters list it, in cluster order. A surface that
/// crosses a portal is listed by each cluster it is in, and Halo 1 draws
/// it when any of them is seen; Halo 2 draws a cluster's geometry only
/// when its flood reaches that cluster, so the surface goes into each
/// (`tests/portal_flood.rs`, `bloodgulch_surfaces_in_two_clusters`: 760
/// of Blood Gulch's 5503 surfaces are listed twice, and drawn with the
/// first cluster only, seen surfaces went missing in 157 of 2556 views
/// from the spawns; drawn with each, in none). Empty for a surface no
/// cluster lists, which Halo 1 never draws. A structure with no clusters
/// has every surface in cluster 0.
pub fn surface_clusters(bsp: &StructureBsp, surfaces: usize) -> Result<Vec<Vec<u16>>> {
    let clusters = bsp.space.block_at(bsp.root, H1_CLUSTERS, H1_CLUSTER)?;
    if clusters.is_empty() {
        return Ok(vec![vec![0]; surfaces]);
    }
    let mut owner: Vec<Vec<u16>> = vec![Vec::new(); surfaces];
    for (c, cluster) in clusters.iter().enumerate() {
        for sub in bsp.space.block_at(cluster, H1_SUBCLUSTERS, H1_SUBCLUSTER)? {
            for ix in bsp.space.block_at(sub, H1_SUBCLUSTER_SURFACES, 4)? {
                if let Some(o) = owner.get_mut(u32_at(ix, 0) as usize) {
                    if o.last() != Some(&(c as u16)) {
                        o.push(c as u16);
                    }
                }
            }
        }
    }
    Ok(owner)
}

/// Texels apart two pieces of one Halo 1 page may be and still be taken as
/// one: each piece has its own border, so pieces this close cost no more
/// as one.
const MERGE_SLACK: u32 = 2 * h1bitmaps::atlas::BORDER;

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Whether texel rectangles `[x, y, w, h]` overlap or lie within
/// [`MERGE_SLACK`] of each other.
fn near(a: [u32; 4], b: [u32; 4]) -> bool {
    (0..2).all(|k| a[k] <= b[k] + b[k + 2] + MERGE_SLACK && b[k] <= a[k] + a[k + 2] + MERGE_SLACK)
}

fn union_rect(a: [u32; 4], b: [u32; 4]) -> [u32; 4] {
    let x = a[0].min(b[0]);
    let y = a[1].min(b[1]);
    let r = (a[0] + a[2]).max(b[0] + b[2]);
    let t = (a[1] + a[3]).max(b[1] + b[3]);
    [x, y, r - x, t - y]
}

/// Gives each cluster with lit surfaces a lightmap page of its own. Each
/// lit mesh's triangles fall into charts (triangles sharing vertices), and
/// each chart takes the texels its lightmap coordinates reach on its Halo 1
/// page ([`Piece::footprint`]); charts of one page that overlap or touch
/// become one piece, so no texel is taken twice. A cluster that also has
/// unlit meshes gets a white piece for them (Halo 1 lights those fully).
/// Moves every mesh's lightmap coordinates onto its cluster's page and sets
/// its `page` to that page's index; a cluster with nothing lit keeps
/// `None`. `page_sizes` are the Halo 1 pages' sizes; a mesh naming a page
/// past them is unlit.
pub fn lightmap_pages(
    meshes: &mut [Mesh],
    page_sizes: &[[u32; 2]],
    clusters: usize,
) -> Result<Vec<ClusterPage>> {
    let mut out = Vec::new();
    for c in 0..clusters {
        let mine: Vec<usize> = (0..meshes.len())
            .filter(|&i| usize::from(meshes[i].cluster) == c)
            .collect();
        let lit_page = |m: &Mesh| m.page.filter(|&p| usize::from(p) < page_sizes.len());
        // Charts: (page, texel rectangle); per mesh, each vertex's chart.
        let mut charts: Vec<(u16, [u32; 4])> = Vec::new();
        let mut chart_of: Vec<Vec<usize>> = vec![Vec::new(); mine.len()];
        let mut unlit = false;
        for (k, &i) in mine.iter().enumerate() {
            let m = &meshes[i];
            let Some(p) = lit_page(m) else {
                unlit = true;
                continue;
            };
            let size = page_sizes[usize::from(p)];
            let mut parent: Vec<usize> = (0..m.vertices.len()).collect();
            for t in &m.triangles {
                let a = find(&mut parent, t[0] as usize);
                for &v in &t[1..] {
                    let b = find(&mut parent, v as usize);
                    parent[b] = a;
                }
            }
            let mut bounds: BTreeMap<usize, ([f32; 2], [f32; 2])> = BTreeMap::new();
            for &v in m.triangles.iter().flatten() {
                let root = find(&mut parent, v as usize);
                let uv = m.vertices[v as usize].lightmap;
                let r = bounds.entry(root).or_insert((uv, uv));
                r.0 = [r.0[0].min(uv[0]), r.0[1].min(uv[1])];
                r.1 = [r.1[0].max(uv[0]), r.1[1].max(uv[1])];
            }
            let mut index = BTreeMap::new();
            for (root, (lo, hi)) in bounds {
                index.insert(root, charts.len());
                charts.push((p, Piece::footprint(size, lo, hi)));
            }
            chart_of[k] = (0..m.vertices.len())
                .map(|v| {
                    let root = find(&mut parent, v);
                    index.get(&root).copied().unwrap_or(usize::MAX)
                })
                .collect();
        }
        if charts.is_empty() {
            for &i in &mine {
                meshes[i].page = None;
            }
            continue;
        }
        // Charts that overlap or touch on one page become one piece.
        let mut parent: Vec<usize> = (0..charts.len()).collect();
        let mut rect: Vec<[u32; 4]> = charts.iter().map(|c| c.1).collect();
        loop {
            let mut changed = false;
            for i in 0..charts.len() {
                for j in i + 1..charts.len() {
                    if charts[i].0 != charts[j].0 {
                        continue;
                    }
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b && near(rect[a], rect[b]) {
                        rect[a] = union_rect(rect[a], rect[b]);
                        parent[b] = a;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let mut piece_of_root = BTreeMap::new();
        let mut pieces: Vec<Piece> = Vec::new();
        for i in 0..charts.len() {
            let root = find(&mut parent, i);
            piece_of_root.entry(root).or_insert_with(|| {
                let p = charts[root].0;
                pieces.push(Piece {
                    page: Some(p),
                    page_size: page_sizes[usize::from(p)],
                    from: rect[root],
                    at: [0; 2],
                });
                pieces.len() - 1
            });
        }
        let white = unlit.then(|| {
            pieces.push(Piece {
                page: None,
                page_size: [2, 2],
                from: [0, 0, 2, 2],
                at: [0; 2],
            });
            pieces.len() - 1
        });
        let sizes: Vec<[u32; 2]> = pieces.iter().map(Piece::size).collect();
        let (size, at) = h1bitmaps::atlas::pack(&sizes).ok_or(Error::TooBig {
            what: "cluster lightmap side",
            value: sizes.iter().map(|s| s[0].max(s[1])).max().unwrap_or(0),
            max: h1bitmaps::atlas::MAX_SIDE,
        })?;
        for (p, a) in pieces.iter_mut().zip(at) {
            p.at = a;
        }
        let image = u16::try_from(out.len()).map_err(|_| Error::TooBig {
            what: "lightmap pages",
            value: out.len() as u32,
            max: u16::MAX as u32,
        })?;
        for (k, &i) in mine.iter().enumerate() {
            let m = &mut meshes[i];
            let lit = lit_page(m).is_some();
            for (v, vertex) in m.vertices.iter_mut().enumerate() {
                let piece = if lit {
                    match chart_of[k].get(v) {
                        Some(&chart) if chart != usize::MAX => {
                            piece_of_root[&find(&mut parent, chart)]
                        }
                        // A vertex no triangle uses: left where it is.
                        _ => continue,
                    }
                } else {
                    white.expect("a white piece for unlit meshes")
                };
                vertex.lightmap = pieces[piece].map(vertex.lightmap, size);
            }
            m.page = Some(image);
        }
        out.push(ClusterPage { size, pieces });
    }
    Ok(out)
}

/// One section per cluster, in cluster order: each cluster's meshes
/// packed as [`crate::structure::pack`] packs them, an empty section for a
/// cluster with none. Fails when a cluster's meshes need more than one
/// section (more than [`SECTION_MAX`] vertices or indices), since a cluster
/// draws one.
pub fn pack_clusters(meshes: &[Mesh], material: &[u16], clusters: usize) -> Result<Vec<Section>> {
    let mut out = Vec::with_capacity(clusters);
    for c in 0..clusters {
        let mine: Vec<usize> = (0..meshes.len())
            .filter(|&i| usize::from(meshes[i].cluster) == c)
            .collect();
        let sections = crate::structure::pack_order(meshes, material, mine);
        match sections.len() {
            0 => out.push(Section::default()),
            1 => out.extend(sections),
            _ => {
                return Err(Error::TooBig {
                    what: "vertices in a cluster",
                    value: sections.iter().map(|s| s.vertices.len() as u32).sum(),
                    max: SECTION_MAX as u32,
                });
            }
        }
    }
    Ok(out)
}

/// A portal between two clusters (Halo 2 `sbsp +0x5c` element).
#[derive(Debug, Clone, PartialEq)]
pub struct Portal {
    /// The cluster behind the plane (`+0x00`) and the one in front
    /// (`+0x02`): the flood crosses from the first when the camera is
    /// behind the plane (E `0x001738e5`, `0x00172953`). Halo 1 orders them
    /// the same way (V, by cluster centres on both discs:
    /// docs/halo1/multiplayer.md, "Clusters and portals").
    pub clusters: [u16; 2],
    /// The collision BSP plane it lies in (`+0x04`); the converted
    /// collision BSP keeps Halo 1's planes in order.
    pub plane: u32,
    /// Centre (`+0x08`) and bounding radius (`+0x14`).
    pub centroid: [f32; 3],
    pub radius: f32,
    /// Its polygon (`+0x1c` block, 12 bytes each).
    pub vertices: Vec<[f32; 3]>,
}

/// The converted structure's portals and each cluster's portal list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Visibility {
    pub portals: Vec<Portal>,
    /// Per cluster: the portals on its boundary.
    pub clusters: Vec<Vec<u16>>,
    /// Per cluster: the sky it shows, an index into the scenario's skies
    /// ([`cluster_skies`]). Empty: every cluster shows sky 0.
    pub skies: Vec<i8>,
}

impl Visibility {
    /// The `sbsp +0x5c` elements: flags (`+0x18`) zero, every portal open
    /// (bit 3 would start it closed, E `0x00210e43`).
    pub fn portal_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; PORTAL * self.portals.len()];
        for (p, b) in self.portals.iter().zip(out.as_chunks_mut::<PORTAL>().0) {
            b[0..2].copy_from_slice(&p.clusters[0].to_le_bytes());
            b[2..4].copy_from_slice(&p.clusters[1].to_le_bytes());
            b[4..8].copy_from_slice(&p.plane.to_le_bytes());
            for (k, v) in p.centroid.iter().chain([&p.radius]).enumerate() {
                b[8 + 4 * k..12 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
        }
        out
    }
}

/// Halo 1's portals and cluster portal lists, checked: each portal joins
/// two of the `clusters` clusters in one of the `planes` planes, each list
/// names portals there are, and there are at most [`MAX_PORTALS`].
pub fn portals(bsp: &StructureBsp, clusters: usize, planes: usize) -> Result<Visibility> {
    let elements = bsp.space.block_at(bsp.root, H1_PORTALS, H1_PORTAL)?;
    if elements.len() > MAX_PORTALS {
        return Err(Error::TooBig {
            what: "portals",
            value: elements.len() as u32,
            max: MAX_PORTALS as u32,
        });
    }
    let mut portals = Vec::with_capacity(elements.len());
    for e in &elements {
        let pair = [i16_at(e, 0), i16_at(e, 2)];
        let clusters = pair.map(|c| u16::try_from(c).ok().filter(|&c| usize::from(c) < clusters));
        let [Some(a), Some(b)] = clusters else {
            return Err(Error::Invalid("a portal names a cluster there isn't"));
        };
        let plane = u32_at(e, 4);
        if plane as usize >= planes {
            return Err(Error::Invalid("a portal names a plane there isn't"));
        }
        let vertices = bsp
            .space
            .block_at(e, H1_PORTAL_VERTICES, 12)?
            .iter()
            .map(|v| [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)])
            .collect();
        portals.push(Portal {
            clusters: [a, b],
            plane,
            centroid: [f32_at(e, 8), f32_at(e, 12), f32_at(e, 16)],
            radius: f32_at(e, 20),
            vertices,
        });
    }
    let mut lists = Vec::with_capacity(clusters);
    for c in bsp.space.block_at(bsp.root, H1_CLUSTERS, H1_CLUSTER)? {
        let mut list = Vec::new();
        for p in bsp.space.block_at(c, H1_CLUSTER_PORTALS, 2)? {
            let p = u16::try_from(i16_at(p, 0))
                .ok()
                .filter(|&p| usize::from(p) < portals.len())
                .ok_or(Error::Invalid("a cluster names a portal there isn't"))?;
            list.push(p);
        }
        lists.push(list);
    }
    lists.resize(clusters, Vec::new());
    let skies = cluster_skies(bsp, &portals, clusters)?;
    Ok(Visibility {
        portals,
        clusters: lists,
        skies,
    })
}

/// A Halo 1 cluster's sky (`+0x00`, i16): an index into the scenario's
/// skies, -1 none (`tests/portal_flood.rs`,
/// `halo1_clusters_name_their_sky`).
pub const H1_CLUSTER_SKY: usize = 0x00;

/// The sky each converted cluster shows (cluster `+0x6e`). Halo 2 draws
/// one sky, the camera's cluster's (E `0x0002b865` calls `0x0014b360`,
/// which reads `+0x6e` of the camera's cluster), where Halo 1 draws the
/// sky of the clusters it sees. So a cluster keeps Halo 1's sky, and one
/// with none (an interior) takes the sky of the nearest cluster through
/// portals that has one: from inside, a window still shows the sky
/// outside. Clusters no sky reaches, and structures with no clusters,
/// show sky 0.
pub fn cluster_skies(bsp: &StructureBsp, portals: &[Portal], clusters: usize) -> Result<Vec<i8>> {
    let own: Vec<i8> = bsp
        .space
        .block_at(bsp.root, H1_CLUSTERS, H1_CLUSTER)?
        .iter()
        .map(|c| i8::try_from(i16_at(c, H1_CLUSTER_SKY)).ok().filter(|&s| s >= 0).unwrap_or(-1))
        .collect();
    Ok(spread_skies(own, portals, clusters))
}

/// [`cluster_skies`] over `own` (Halo 1's, -1 none): breadth first from
/// every cluster with a sky, so each cluster without one takes its
/// nearest's (fewest portals; on a tie, the one
/// the search meets first).
pub fn spread_skies(mut own: Vec<i8>, portals: &[Portal], clusters: usize) -> Vec<i8> {
    own.resize(clusters, -1);
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); clusters];
    for p in portals {
        let [a, b] = p.clusters.map(usize::from);
        if a < clusters && b < clusters {
            next[a].push(b);
            next[b].push(a);
        }
    }
    let mut queue: std::collections::VecDeque<usize> = (0..clusters).filter(|&c| own[c] >= 0).collect();
    while let Some(c) = queue.pop_front() {
        let sky = own[c];
        for &o in &next[c] {
            if own[o] < 0 {
                own[o] = sky;
                queue.push_back(o);
            }
        }
    }
    own.into_iter().map(|s| s.max(0)).collect()
}

/// Each converted cluster's sound zone and each collision leaf's cluster:
/// Halo 1's own. A cluster's background sound `+0x04` and sound
/// environment `+0x06` (i16 into the BSP's palettes; V: every Halo 1
/// level, `tests/h1_clusters.rs`) are mapped by `index` to the converted
/// palettes; a leaf (`sbsp +0xe0`, one per collision leaf) names its
/// cluster at `+0x08`. A leaf naming no cluster, or leaves that don't run
/// parallel to the `collision_leaves`, are in cluster 0.
pub fn sound_zones(
    src: &StructureBsp,
    collision_leaves: usize,
    index: impl Fn([i16; 2]) -> [i16; 2],
) -> Result<(Vec<[i16; 2]>, Vec<u16>)> {
    let mut zones: Vec<[i16; 2]> = src
        .space
        .block_at(src.root, H1_CLUSTERS, H1_CLUSTER)?
        .iter()
        .map(|c| index([i16_at(c, 4), i16_at(c, 6)]))
        .collect();
    if zones.is_empty() {
        zones.push([-1, -1]);
    }
    let leaves = src.space.block_at(src.root, H1_LEAVES, H1_LEAF)?;
    let per_leaf = if leaves.len() == collision_leaves {
        leaves
            .iter()
            .map(|l| {
                u16::try_from(i16_at(l, 8))
                    .ok()
                    .filter(|&c| usize::from(c) < zones.len())
                    .unwrap_or(0)
            })
            .collect()
    } else {
        vec![0; collision_leaves]
    };
    Ok((zones, per_leaf))
}

/// Splits a material's triangles by the clusters of the surface each is
/// (`clusters[first + k]` for triangle `k`; a triangle goes into each of
/// its clusters, [`surface_clusters`]), dropping those no cluster lists:
/// one mesh per cluster, each with the material's vertices.
pub fn split_by_cluster(
    shader: &str,
    page: Option<u16>,
    vertices: Vec<Vertex>,
    triangles: &[[u32; 3]],
    clusters: &[Vec<u16>],
) -> Vec<Mesh> {
    let mut by: BTreeMap<u16, Vec<[u32; 3]>> = BTreeMap::new();
    for (t, cs) in triangles.iter().zip(clusters) {
        for c in cs {
            by.entry(*c).or_default().push(*t);
        }
    }
    by.into_iter()
        .map(|(cluster, triangles)| Mesh {
            shader: shader.to_owned(),
            page,
            cluster,
            vertices: vertices.clone(),
            triangles,
        })
        .collect()
}

/// A structure's render geometry laid out by cluster.
#[derive(Debug, Clone, Default)]
pub struct Laid {
    /// One section per Halo 1 cluster.
    pub sections: Vec<Section>,
    /// The portals and each cluster's portal list.
    pub visibility: Visibility,
    /// Each lit cluster's lightmap page, in the order the sections'
    /// `page` index them.
    pub pages: Vec<ClusterPage>,
}

/// Lays out `meshes` ([`crate::structure::meshes`], `material[i]` mesh
/// `i`'s structure material) by Halo 1 cluster: each lit cluster's
/// lightmap page ([`lightmap_pages`], over the Halo 1 lightmap bitmap's
/// page sizes), one section per cluster ([`pack_clusters`]) and the
/// portals ([`portals`]; `planes` is the collision BSP's plane count).
pub fn lay_out(
    h1: &H1Map,
    src: &StructureBsp,
    meshes: &mut [Mesh],
    material: &[u16],
    planes: usize,
) -> Result<Laid> {
    let clusters = cluster_count(src)?;
    let bitmap = src.lightmaps_bitmap();
    let page_sizes: Vec<[u32; 2]> = if bitmap.id.is_none() {
        Vec::new()
    } else {
        let tag = h1.tag(bitmap.id)?;
        Bitmap::read(h1, tag)?
            .images
            .iter()
            .map(|i| [u32::from(i.width), u32::from(i.height)])
            .collect()
    };
    let pages = lightmap_pages(meshes, &page_sizes, clusters)?;
    Ok(Laid {
        sections: pack_clusters(meshes, material, clusters)?,
        visibility: portals(src, clusters, planes)?,
        pages,
    })
}

/// The converted lightmap bitmap of `pages` ([`h1bitmaps::atlas::compose`]
/// over the structure's Halo 1 lightmap bitmap), with that bitmap's tag
/// name; `None` when the structure has no lightmap bitmap or no cluster is
/// lit.
pub fn lightmap_bitmap(
    h1: &H1Map,
    src: &StructureBsp,
    pages: &[ClusterPage],
) -> Result<Option<(String, Bitmap, Converted)>> {
    let bitmap = src.lightmaps_bitmap();
    if bitmap.id.is_none() || pages.is_empty() {
        return Ok(None);
    }
    let tag = h1.tag(bitmap.id)?;
    let (bm, converted) = h1bitmaps::atlas::compose(h1, tag, pages)?;
    Ok(Some((tag.name.clone(), bm, converted)))
}
