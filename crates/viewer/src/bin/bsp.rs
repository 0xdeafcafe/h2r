//! Structure BSP render geometry and lightmap validation against your own
//! disc, plus an optional software render of one BSP.
//!
//!   bsp [--disc ISO] --all                      validate every scenario map
//!   bsp [--disc ISO] <map>                      validate one map
//!   bsp [--disc ISO] <map> --render DIR [--bsp N] [--force]
//!
//! Prints aggregate counts and failures only. Exits 1 when anything fails
//! (see [`breaches`]). `--render` writes two PNGs into DIR, never through a
//! symlink and never over an existing file without `--force`.
#[path = "../disc.rs"]
#[allow(dead_code)]
mod disc;
#[path = "../out.rs"]
#[allow(dead_code)]
mod out;

use anyhow::{Context, Result, bail};
use disc::{Disc, DiscResources};
use map::{CacheType, Map, ResourceFile, TagId};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tags::geometry::{Section, TagView, VertexKind};
use tags::structure::{GeometryHeader, Lightmap, StructureRender, load_bucket};
use tags::{DecodedTag, TagData};

type MapReader = Map<BufReader<File>>;

/// (bsp, lightmap) pairs from the scenario's BSP block (+0x210, stride 0x44).
fn scenario_bsps(map: &mut MapReader) -> Result<Vec<(TagId, Option<TagId>)>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let mut out = Vec::new();
    for b in view.block(view.at(0, 0x218)?, 0x210, 0x44)? {
        let word = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let lightmap = word(0x1c);
        out.push((
            TagId(word(0x14)),
            (lightmap != u32::MAX).then_some(TagId(lightmap)),
        ));
    }
    Ok(out)
}

#[derive(Default)]
struct Stats {
    maps: usize,
    maps_failed: usize,
    bsps: usize,
    clusters: usize,
    definitions: usize,
    instances: usize,
    sections: usize,
    absent_sections: usize,
    parts: usize,
    drawn_parts: usize,
    triangles: usize,
    vertices: usize,
    stream_kinds: BTreeMap<(String, u8), usize>,
    files: BTreeMap<String, usize>,
    /// Raw blocks outside the map's own file (V: none).
    non_local_blocks: usize,
    outside_bounds: usize,
    bad_normals: usize,
    zero_normals: usize,
    /// Second line of defence; the decoder rejects untiled sections.
    layout_mismatches: usize,
    normals_checked: usize,
    winding_agree: usize,
    winding_disagree: usize,
    lightmap_uvs: usize,
    lightmap_uv_out_of_range: usize,
    non_rigid_placements: usize,
    lightmaps: usize,
    groups: usize,
    checksum_mismatches: usize,
    render_info_checked: usize,
    bucket_sections: usize,
    bucket_vertices: usize,
    bucket_flag_mismatches: usize,
    bucket_refs_checked: usize,
    instance_refs_not_single: usize,
    part_types: BTreeMap<u16, usize>,
    unsupported: BTreeMap<usize, usize>,
    embedded_sections: usize,
    observations: Vec<String>,
    failures: Vec<String>,
}

/// Thresholds for counters that are nonzero on the retail disc (observed
/// values in docs/formats/structure-bsp.md). Every other counter must be zero.
const MAX_ZERO_NORMAL_FRACTION: f64 = 0.002; // observed 12,104 of ~22M
const MAX_WINDING_DISAGREE_FRACTION: f64 = 0.001; // observed 494 of 2.36M
const MAX_CHECKSUM_MISMATCHES: usize = 2; // observed 2 groups

/// Every check that fails, as a readable line. Empty means pass.
fn breaches(st: &Stats) -> Vec<String> {
    let mut out: Vec<String> = st.failures.iter().take(400).cloned().collect();
    if st.failures.len() > 400 {
        out.push(format!("... {} failures in all", st.failures.len()));
    }
    for (n, what) in [
        (st.maps_failed, "maps failed to open"),
        (st.layout_mismatches, "part/subpart tiling mismatches"),
        (st.non_local_blocks, "raw blocks outside the local map"),
        (st.outside_bounds, "positions outside cluster bounds"),
        (st.bad_normals, "non-unit packed vectors"),
        (st.lightmap_uv_out_of_range, "lightmap uvs outside [0, 1]"),
        (st.non_rigid_placements, "non-rigid placements"),
        (
            st.bucket_flag_mismatches,
            "bucket flag bit 1 / 0x2f stream mismatches",
        ),
        (
            st.instance_refs_not_single,
            "instance bucket refs without exactly one offset",
        ),
    ] {
        if n != 0 {
            out.push(format!("{n} {what} (must be 0)"));
        }
    }
    let frac = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 / d as f64 };
    if frac(st.zero_normals, st.normals_checked) > MAX_ZERO_NORMAL_FRACTION {
        out.push(format!(
            "{}/{} zero packed vectors (max fraction {MAX_ZERO_NORMAL_FRACTION})",
            st.zero_normals, st.normals_checked
        ));
    }
    let windings = st.winding_agree + st.winding_disagree;
    if frac(st.winding_disagree, windings) > MAX_WINDING_DISAGREE_FRACTION {
        out.push(format!(
            "{}/{windings} triangles wind against their normal (max fraction {MAX_WINDING_DISAGREE_FRACTION})",
            st.winding_disagree
        ));
    }
    if st.checksum_mismatches > MAX_CHECKSUM_MISMATCHES {
        out.push(format!(
            "{} lightmap groups with another BSP's checksum (max {MAX_CHECKSUM_MISMATCHES})",
            st.checksum_mismatches
        ));
    }
    out
}

fn kind_name(k: VertexKind) -> String {
    match k {
        VertexKind::Unknown(_) => "unknown".into(),
        k => format!("{k:?}"),
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn check_section(s: &Section, bounds: Option<[f32; 6]>, st: &mut Stats) -> Result<()> {
    st.sections += 1;
    st.parts += s.parts.len();
    st.drawn_parts += s.parts.iter().filter(|p| p.is_drawn()).count();
    let mut next = 0usize;
    for p in &s.parts {
        *st.part_types.entry(p.kind).or_default() += 1;
        st.layout_mismatches += usize::from(p.index_range().start != next);
        next = p.index_range().end;
    }
    st.layout_mismatches += usize::from(next != s.indices.len());
    st.vertices += s.vertex_count();
    for v in &s.vertex_buffers {
        *st.stream_kinds
            .entry((kind_name(v.kind), v.type_id))
            .or_default() += 1;
    }
    let v = s.vertices(None)?;
    let p = &v.positions;
    if let Some(b) = bounds {
        let eps = 0.01;
        st.outside_bounds += p
            .iter()
            .filter(|v| (0..3).any(|k| v[k] < b[2 * k] - eps || v[k] > b[2 * k + 1] + eps))
            .count();
    }
    for list in [&v.normals, &v.binormals, &v.tangents] {
        for n in list {
            st.normals_checked += 1;
            let len = dot(*n, *n).sqrt();
            if len < 0.02 {
                st.zero_normals += 1;
            } else if (len - 1.0).abs() > 0.02 {
                st.bad_normals += 1;
            }
        }
    }
    for part in &s.parts {
        let tris = s.part_triangles(part);
        st.triangles += tris.len();
        if v.normals.is_empty() {
            continue;
        }
        for t in tris {
            let [a, b, c] = t.map(|i| p[usize::from(i)]);
            let n = cross(sub(b, a), sub(c, a));
            if dot(n, n) < 1e-10 {
                continue;
            }
            if dot(n, v.normals[usize::from(t[0])]) >= 0.0 {
                st.winding_agree += 1;
            } else {
                st.winding_disagree += 1;
            }
        }
    }
    st.lightmap_uvs += v.lightmap_texcoords.len();
    st.lightmap_uv_out_of_range += v
        .lightmap_texcoords
        .iter()
        .filter(|t| t.iter().any(|&c| !(-0.001..=1.001).contains(&c)))
        .count();
    Ok(())
}

/// Load one geometry header and account for it. Returns the section's
/// lightmap vertex need (`unknown_0c`), 0 when absent or failed.
fn account_geometry(
    g: &GeometryHeader,
    bounds: Option<[f32; 6]>,
    materials: Option<usize>,
    raw: &mut DiscResources,
    st: &mut Stats,
    ctx: &str,
) -> usize {
    if let Some(p) = g.block.block {
        *st.files.entry(format!("{:?}", p.file)).or_default() += 1;
        st.non_local_blocks += usize::from(p.file != ResourceFile::Local);
    }
    if g.info.unknown_0c > g.info.total_vertex_count {
        st.failures.push(format!("{ctx}: +0xc count exceeds total"));
    }
    st.embedded_sections += usize::from(g.embedded_section.is_some());
    match g.load(raw, materials) {
        Ok(Some(s)) => {
            if let Err(e) = check_section(&s, bounds, st) {
                st.failures.push(format!("{ctx}: {e}"));
                return 0;
            }
            usize::from(g.info.unknown_0c)
        }
        Ok(None) => {
            st.absent_sections += 1;
            0
        }
        Err(e) => {
            st.failures.push(format!("{ctx}: {e}"));
            0
        }
    }
}

fn validate_map(disc: &Disc, name: &str, st: &mut Stats) -> Result<()> {
    let mut map = disc.open_map(name)?;
    let mut raw = disc.resources(name)?;
    st.maps += 1;
    for (bsp_id, lightmap_id) in scenario_bsps(&mut map)? {
        let ctx = |what: &str| format!("{name} {bsp_id} {what}");
        let bsp = match StructureRender::read(&mut map, bsp_id) {
            Ok(b) => b,
            Err(e) => {
                st.failures.push(format!("{}: {e}", ctx("bsp")));
                continue;
            }
        };
        st.bsps += 1;
        st.clusters += bsp.clusters.len();
        st.definitions += bsp.instance_definitions.len();
        st.instances += bsp.instances.len();
        st.non_rigid_placements += bsp
            .instances
            .iter()
            .filter(|i| !i.is_rigid() || i.scale <= 0.0)
            .count();
        let materials = Some(bsp.material_count as usize);
        for (i, c) in bsp.clusters.iter().enumerate() {
            account_geometry(
                &c.geometry,
                Some(c.bounds),
                materials,
                &mut raw,
                st,
                &ctx(&format!("cluster {i}")),
            );
        }
        // One entry per definition on every path, so indices stay aligned.
        let def_vertices: Vec<usize> = bsp
            .instance_definitions
            .iter()
            .enumerate()
            .map(|(i, d)| {
                account_geometry(
                    &d.geometry,
                    None,
                    materials,
                    &mut raw,
                    st,
                    &ctx(&format!("definition {i}")),
                )
            })
            .collect();
        let Some(lightmap_id) = lightmap_id else {
            continue;
        };
        let lightmap = match Lightmap::read(&mut map, lightmap_id) {
            Ok(l) => l,
            Err(e) => {
                st.failures.push(format!("{}: {e}", ctx("lightmap")));
                continue;
            }
        };
        st.lightmaps += 1;
        for (gi, g) in lightmap.groups.iter().enumerate() {
            st.groups += 1;
            let gctx = |e: &str| format!("{name} {lightmap_id} group {gi}: {e}");
            for (k, h) in g
                .cluster_geometry
                .iter()
                .chain(&g.instance_geometry)
                .enumerate()
            {
                account_geometry(h, None, None, &mut raw, st, &gctx(&format!("geometry {k}")));
            }
            if g.structure_checksum != bsp.checksum {
                // Not a decoding failure: this group was built for another
                // BSP revision. Its per-cluster tables cannot be checked.
                st.checksum_mismatches += 1;
                st.observations.push(gctx(&format!(
                    "structure checksum {:#x} differs from BSP {:#x}",
                    g.structure_checksum, bsp.checksum
                )));
                continue;
            }
            for (k, &c) in g.unsupported_counts.iter().enumerate() {
                if c != 0 {
                    *st.unsupported.entry([0x10, 0x38, 0x58][k]).or_default() += 1;
                }
            }
            if g.cluster_render_info.len() != bsp.clusters.len()
                || g.instance_render_info.len() != bsp.instances.len()
                || (!g.instance_bucket_refs.is_empty()
                    && g.instance_bucket_refs.len() != bsp.instances.len())
            {
                st.failures.push(gctx("render info counts differ from BSP"));
            }
            let images = match g.bitmap {
                Some(id) => match DecodedTag::read(&mut map, id) {
                    Ok(DecodedTag {
                        data: TagData::Bitmap(b),
                        ..
                    }) => b.images.len(),
                    Ok(_) => 0,
                    Err(e) => {
                        st.failures.push(gctx(&format!("bitmap: {e}")));
                        0
                    }
                },
                None => 0,
            };
            for r in g.cluster_render_info.iter().chain(&g.instance_render_info) {
                st.render_info_checked += 1;
                let bitmap_ok = r.bitmap_index == -1
                    || (r.bitmap_index >= 0 && (r.bitmap_index as usize) < images);
                let palette_ok = r.palette_index == -1
                    || (r.palette_index >= 0 && (r.palette_index as usize) < g.palettes.len());
                if !bitmap_ok || !palette_ok {
                    st.failures.push(gctx(&format!(
                        "render info {r:?} outside {images} images / {} palettes",
                        g.palettes.len()
                    )));
                    break;
                }
            }
            let mut bucket_counts = Vec::with_capacity(g.buckets.len());
            for (bi, b) in g.buckets.iter().enumerate() {
                if let Some(p) = b.block.block {
                    *st.files.entry(format!("{:?}", p.file)).or_default() += 1;
                    st.non_local_blocks += usize::from(p.file != ResourceFile::Local);
                }
                let n = match load_bucket(b, &mut raw) {
                    Ok(Some(s)) => {
                        st.bucket_sections += 1;
                        st.bucket_vertices += s.vertex_count();
                        for v in &s.vertex_buffers {
                            *st.stream_kinds
                                .entry((format!("bucket {}", kind_name(v.kind)), v.type_id))
                                .or_default() += 1;
                        }
                        let has_2f = s.stream(VertexKind::Unknown(0x2f)).is_some();
                        st.bucket_flag_mismatches += usize::from(has_2f != (b.flags & 2 != 0));
                        s.vertex_count()
                    }
                    Ok(None) => 0,
                    Err(e) => {
                        st.failures.push(gctx(&format!("bucket {bi}: {e}")));
                        0
                    }
                };
                bucket_counts.push(n);
            }
            // A placement's lightmap vertices must fit its bucket. Only
            // buckets with flag bit 0 are patched by the engine.
            for (ii, r) in g.instance_bucket_refs.iter().enumerate() {
                let Some(inst) = bsp.instances.get(ii) else {
                    break;
                };
                st.instance_refs_not_single += usize::from(r.section_offsets.len() != 1);
                let bucket = usize::from(r.bucket);
                if !g.buckets[bucket].is_patched() {
                    continue;
                }
                let need = def_vertices[inst.definition as usize];
                if let Some(o) = r.section_offsets[0] {
                    st.bucket_refs_checked += 1;
                    if usize::from(o) + need > bucket_counts[bucket] {
                        st.failures.push(gctx(&format!(
                            "instance {ii}: offset {o} + {need} exceeds bucket {bucket} ({} vertices)",
                            bucket_counts[bucket]
                        )));
                    }
                }
            }
            // Cluster references: which offset the engine reads depends on a
            // render key, so each one must at least lie inside its bucket.
            for (ci, r) in g.cluster_bucket_refs.iter().enumerate() {
                let bucket = usize::from(r.bucket);
                if !g.buckets[bucket].is_patched() {
                    continue;
                }
                for &o in r.section_offsets.iter().flatten() {
                    st.bucket_refs_checked += 1;
                    if usize::from(o) >= bucket_counts[bucket] {
                        st.failures.push(gctx(&format!(
                            "cluster ref {ci}: offset {o} outside bucket {bucket} ({} vertices)",
                            bucket_counts[bucket]
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn scenario_map(disc: &Disc, name: &str) -> Result<bool> {
    let kind = disc.open_map(name)?.header().cache_type;
    Ok(matches!(
        kind,
        CacheType::Campaign | CacheType::Multiplayer | CacheType::MainMenu
    ))
}

fn run_maps(disc: &Disc, names: &[String]) -> bool {
    let mut st = Stats::default();
    for name in names {
        match scenario_map(disc, name) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                st.maps_failed += 1;
                st.failures.push(format!("{name}: cannot open: {e:#}"));
                continue;
            }
        }
        let before = (
            st.bsps,
            st.clusters,
            st.instances,
            st.triangles,
            st.failures.len(),
        );
        if let Err(e) = validate_map(disc, name, &mut st) {
            st.maps_failed += 1;
            st.failures.push(format!("{name}: {e:#}"));
        }
        println!(
            "{name:<24} bsps {:>2} clusters {:>4} instances {:>5} triangles {:>8} failures {}",
            st.bsps - before.0,
            st.clusters - before.1,
            st.instances - before.2,
            st.triangles - before.3,
            st.failures.len() - before.4
        );
    }
    report(&st);
    let breached = breaches(&st);
    for b in &breached {
        println!("CHECK FAILED: {b}");
    }
    println!("{}", if breached.is_empty() { "PASS" } else { "FAIL" });
    breached.is_empty()
}

fn report(st: &Stats) {
    println!("---");
    println!(
        "{} maps ({} failed), {} BSPs: {} clusters, {} instanced definitions, {} instances",
        st.maps, st.maps_failed, st.bsps, st.clusters, st.definitions, st.instances
    );
    println!(
        "sections: {} loaded, {} without a raw block, {} in-tag; {} parts ({} drawn), {} triangles, {} vertices",
        st.sections,
        st.absent_sections,
        st.embedded_sections,
        st.parts,
        st.drawn_parts,
        st.triangles,
        st.vertices
    );
    println!("part kinds: {:?}", st.part_types);
    println!("raw blocks by file: {:?}", st.files);
    println!("vertex streams by (kind, type id):");
    for ((k, t), n) in &st.stream_kinds {
        println!("  {k:<28} {t:#04x} {n}");
    }
    println!(
        "checks: tiling mismatches {}; non-local blocks {}; positions outside cluster bounds {}; \
         packed vectors {} ({} zero, {} other non-unit); winding vs first-vertex normal {} agree / {} disagree; \
         lightmap uvs {} ({} outside [0,1]); non-rigid placements {}",
        st.layout_mismatches,
        st.non_local_blocks,
        st.outside_bounds,
        st.normals_checked,
        st.zero_normals,
        st.bad_normals,
        st.winding_agree,
        st.winding_disagree,
        st.lightmap_uvs,
        st.lightmap_uv_out_of_range,
        st.non_rigid_placements
    );
    println!(
        "lightmaps: {} tags, {} groups ({} with another checksum), {} render infos in range, \
         {} bucket sections ({} vertices, {} flag mismatches), {} bucket offsets checked, \
         {} instance refs without one offset",
        st.lightmaps,
        st.groups,
        st.checksum_mismatches,
        st.render_info_checked,
        st.bucket_sections,
        st.bucket_vertices,
        st.bucket_flag_mismatches,
        st.bucket_refs_checked,
        st.instance_refs_not_single
    );
    println!(
        "lightmap groups with unreviewed nonempty blocks (group offset: groups): {}",
        st.unsupported
            .iter()
            .map(|(o, n)| format!("{o:#x}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("observations: {}", st.observations.len());
    for o in &st.observations {
        println!("  {o}");
    }
    println!("failures: {}", st.failures.len());
}

// ---- Offscreen software render -------------------------------------------

/// A world-space triangle, flagged true for instanced geometry.
type WorldTriangle = ([[f32; 3]; 3], bool);

/// World-space triangles of every drawn part of one BSP.
fn world_triangles(disc: &Disc, name: &str, bsp_index: usize) -> Result<Vec<WorldTriangle>> {
    let mut map = disc.open_map(name)?;
    let mut raw = disc.resources(name)?;
    let (bsp_id, _) = *scenario_bsps(&mut map)?
        .get(bsp_index)
        .context("no such BSP")?;
    let bsp = StructureRender::read(&mut map, bsp_id)?;
    let materials = Some(bsp.material_count as usize);
    let mut tris = Vec::new();
    let push = |s: &Section,
                f: &dyn Fn([f32; 3]) -> [f32; 3],
                inst: bool,
                out: &mut Vec<_>|
     -> Result<()> {
        let p = s.vertices(None)?.positions;
        for part in s.parts.iter().filter(|p| p.is_drawn()) {
            for t in s.part_triangles(part) {
                out.push((t.map(|i| f(p[usize::from(i)])), inst));
            }
        }
        Ok(())
    };
    for c in &bsp.clusters {
        if let Some(s) = c.geometry.load(&mut raw, materials)? {
            push(&s, &|v| v, false, &mut tris)?;
        }
    }
    let defs: Vec<Option<Section>> = bsp
        .instance_definitions
        .iter()
        .map(|d| d.geometry.load(&mut raw, materials))
        .collect::<Result<_, _>>()?;
    for i in &bsp.instances {
        if let Some(s) = &defs[i.definition as usize] {
            push(s, &|v| i.transform(v), true, &mut tris)?;
        }
    }
    Ok(tris)
}

struct Image {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
    depth: Vec<f32>,
}
impl Image {
    fn new(w: usize, h: usize) -> Self {
        let mut rgba = vec![0; w * h * 4];
        for y in 0..h {
            let g = 18 + (y * 20 / h) as u8;
            for x in 0..w {
                rgba[(y * w + x) * 4..][..4].copy_from_slice(&[g, g, g + 6, 255]);
            }
        }
        Self {
            w,
            h,
            rgba,
            depth: vec![f32::INFINITY; w * h],
        }
    }
    /// Screen-space vertices (x, y in pixels, z = depth, smaller is nearer).
    fn triangle(&mut self, v: [[f32; 3]; 3], color: [u8; 3]) {
        let span = |k: usize, hi: f32| {
            let lo = v.iter().map(|p| p[k]).fold(f32::INFINITY, f32::min);
            let top = v.iter().map(|p| p[k]).fold(f32::NEG_INFINITY, f32::max);
            (lo.floor().max(0.0) as i64, top.ceil().min(hi - 1.0) as i64)
        };
        let (min_x, max_x) = span(0, self.w as f32);
        let (min_y, max_y) = span(1, self.h as f32);
        let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
            (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])
        };
        let area = edge(v[0], v[1], v[2][0], v[2][1]);
        if area.abs() < 1e-9 {
            return;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(v[1], v[2], px, py) / area;
                let w1 = edge(v[2], v[0], px, py) / area;
                let w2 = edge(v[0], v[1], px, py) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * v[0][2] + w1 * v[1][2] + w2 * v[2][2];
                let i = y as usize * self.w + x as usize;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    self.rgba[i * 4..i * 4 + 3].copy_from_slice(&color);
                }
            }
        }
    }
    fn write_png(&self, path: &Path, force: bool) -> Result<()> {
        out::write_with(path, force, |w| {
            assets::png::write_rgba(w, self.w as u32, self.h as u32, &self.rgba)
        })
        .with_context(|| format!("write {}", path.display()))
    }
}

fn normal_color(n: [f32; 3], inst: bool) -> [u8; 3] {
    let c = n.map(|v| ((v * 0.5 + 0.5) * 255.0) as u8);
    if inst {
        [c[0] / 2 + 100, c[1] / 2 + 60, c[2] / 3]
    } else {
        c
    }
}

fn render(disc: &Disc, name: &str, bsp_index: usize, dir: &Path, force: bool) -> Result<()> {
    let tris = world_triangles(disc, name, bsp_index)?;
    out::ensure_dir(dir)?;
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    // Frame the dense playable core rather than the enclosing shell: the
    // 1st..99th percentile of triangle centroids on each axis.
    for k in 0..3 {
        let mut c: Vec<f32> = tris
            .iter()
            .map(|(t, _)| (t[0][k] + t[1][k] + t[2][k]) / 3.0)
            .collect();
        c.sort_by(f32::total_cmp);
        if !c.is_empty() {
            lo[k] = c[c.len() / 100];
            hi[k] = c[c.len() * 99 / 100];
        }
    }
    if tris.is_empty() {
        bail!("BSP {bsp_index} of {name} has no drawn triangles");
    }
    println!("{} triangles; framing {lo:?} .. {hi:?}", tris.len());
    let (w, h) = (1600usize, 1200usize);

    // Top-down orthographic (+z up, looking down), coloured by face normal.
    let mut img = Image::new(w, h);
    let span = ((hi[0] - lo[0]) / w as f32).max((hi[1] - lo[1]) / h as f32) * 1.05;
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    for (t, inst) in &tris {
        let n = normalize(cross(sub(t[1], t[0]), sub(t[2], t[0])));
        // Back-face cull (counter-clockwise front faces, as the packed
        // normals confirm), so the enclosing shell does not hide the level.
        if n[2] <= 0.0 {
            continue;
        }
        let s = t.map(|p| {
            [
                (p[0] - cx) / span + w as f32 / 2.0,
                h as f32 / 2.0 - (p[1] - cy) / span,
                -p[2],
            ]
        });
        img.triangle(s, normal_color(n, *inst));
    }
    let top = dir.join(format!("{name}_bsp{bsp_index}_top_normals.png"));
    img.write_png(&top, force)?;

    // Perspective, Lambert shaded, from above one corner looking at the centre.
    let centre = [0, 1, 2].map(|k| (lo[k] + hi[k]) / 2.0);
    let radius = dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0;
    let eye = [
        centre[0] - radius * 0.9,
        centre[1] - radius * 1.1,
        centre[2] + radius * 0.8,
    ];
    let fwd = normalize(sub(centre, eye));
    let right = normalize(cross(fwd, [0.0, 0.0, 1.0]));
    let up = cross(right, fwd);
    let focal = h as f32 / 2.0 / (35f32.to_radians()).tan();
    let light = normalize([0.4, 0.3, 0.85]);
    let mut img = Image::new(w, h);
    for (t, inst) in &tris {
        let n = normalize(cross(sub(t[1], t[0]), sub(t[2], t[0])));
        let cam = t.map(|p| {
            let d = sub(p, eye);
            [dot(d, right), dot(d, up), dot(d, fwd)]
        });
        if cam.iter().any(|c| c[2] < 0.1) || dot(n, sub(t[0], eye)) >= 0.0 {
            continue;
        }
        let s = cam.map(|c| {
            [
                w as f32 / 2.0 + focal * c[0] / c[2],
                h as f32 / 2.0 - focal * c[1] / c[2],
                c[2],
            ]
        });
        let l = 0.3 + 0.7 * dot(n, light).max(0.0);
        let base = if *inst {
            [235.0, 190.0, 140.0]
        } else {
            [200.0, 210.0, 225.0]
        };
        img.triangle(s, base.map(|b: f32| (b * l) as u8));
    }
    let persp = dir.join(format!("{name}_bsp{bsp_index}_perspective.png"));
    img.write_png(&persp, force)?;
    println!("wrote {}\nwrote {}", top.display(), persp.display());
    Ok(())
}

const USAGE: &str = "usage: bsp [--disc ISO] (--all | <map> [--render DIR [--bsp N] [--force]])";

fn main() -> Result<()> {
    let mut disc_arg: Option<PathBuf> = None;
    let (mut all, mut force) = (false, false);
    let mut render_dir: Option<PathBuf> = None;
    let mut bsp = 0usize;
    let mut positional = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut next = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--disc" => disc_arg = Some(next()?.into()),
            "--all" => all = true,
            "--force" => force = true,
            "--render" => render_dir = Some(next()?.into()),
            "--bsp" => bsp = next()?.parse()?,
            "-h" | "--help" => bail!("{USAGE}"),
            _ => positional.push(a),
        }
    }
    let disc = Disc::open(&disc::disc_path(disc_arg)?)?;
    let ok = if all {
        run_maps(&disc, &disc.map_names())
    } else {
        let [name] = positional.as_slice() else {
            bail!("{USAGE}");
        };
        if let Some(dir) = render_dir {
            return render(&disc, name, bsp, &dir, force);
        }
        run_maps(&disc, std::slice::from_ref(name))
    };
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Stats {
        Stats {
            normals_checked: 10_000,
            winding_agree: 10_000,
            ..Default::default()
        }
    }

    #[test]
    fn clean_stats_pass() {
        assert!(breaches(&clean()).is_empty());
    }

    #[test]
    fn zero_tolerance_counters_fail_the_run() {
        type Set = fn(&mut Stats);
        let cases: [(Set, &str); 10] = [
            (|s| s.maps_failed = 1, "maps failed"),
            (|s| s.layout_mismatches = 1, "tiling"),
            (|s| s.non_local_blocks = 1, "outside the local map"),
            (|s| s.outside_bounds = 1, "cluster bounds"),
            (|s| s.bad_normals = 1, "non-unit"),
            (|s| s.lightmap_uv_out_of_range = 1, "lightmap uvs"),
            (|s| s.non_rigid_placements = 1, "non-rigid"),
            (|s| s.bucket_flag_mismatches = 1, "0x2f"),
            (|s| s.instance_refs_not_single = 1, "exactly one offset"),
            (
                |s| s.failures.push("x: decode error".into()),
                "decode error",
            ),
        ];
        for (set, word) in cases {
            let mut s = clean();
            set(&mut s);
            let b = breaches(&s);
            assert_eq!(b.len(), 1, "{word}: {b:?}");
            assert!(b[0].contains(word), "{b:?}");
        }
    }

    #[test]
    fn thresholded_counters_fail_only_above_their_limit() {
        let mut s = clean();
        s.zero_normals = 20; // 0.2%
        s.winding_disagree = 10; // ~0.1%
        s.checksum_mismatches = MAX_CHECKSUM_MISMATCHES;
        assert!(breaches(&s).is_empty(), "{:?}", breaches(&s));
        s.zero_normals += 1;
        s.winding_disagree += 2;
        s.checksum_mismatches += 1;
        assert_eq!(breaches(&s).len(), 3);
    }

    #[test]
    fn render_output_never_clobbers() {
        // Resolve the temp dir: on macOS it sits under /var, a symlink the writer refuses.
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("h2-bsp-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.png");
        let img = Image::new(2, 2);
        img.write_png(&f, false).unwrap();
        assert!(img.write_png(&f, false).is_err());
        img.write_png(&f, true).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&f, dir.join("y.png")).unwrap();
            assert!(img.write_png(&dir.join("y.png"), true).is_err());
            // A symlinked render directory (or parent) is refused.
            let real = dir.join("real");
            std::fs::create_dir(&real).unwrap();
            std::os::unix::fs::symlink(&real, dir.join("link")).unwrap();
            assert!(out::ensure_dir(&dir.join("link")).is_err());
            assert!(out::ensure_dir(&dir.join("link/deeper")).is_err());
            assert!(img.write_png(&dir.join("link/z.png"), false).is_err());
            assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
        }
    }
}
