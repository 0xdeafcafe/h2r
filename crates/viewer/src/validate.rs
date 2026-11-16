//! `--all`: decode every mode tag of the given maps and report aggregate
//! counts, failures and cross-checks. Prints counts only, no asset data.
//!
//! The run fails (exit 1) when any tag fails to decode, any map fails to
//! open, or any check in [`breaches`] exceeds its threshold.
use crate::disc::Disc;
use anyhow::Result;
use map::{FourCC, TagId};
use std::collections::BTreeMap;
use tags::geometry::VertexKind;
use tags::model::RenderModelGeometry;

#[derive(Default)]
pub struct Totals {
    pub maps_failed: usize,
    pub tags: usize,
    pub failed: usize,
    pub sections: usize,
    pub parts: usize,
    pub vertices: usize,
    pub indices: usize,
    pub triangles: usize,
    /// Sections whose index-derived triangle count differs from the header.
    pub triangle_mismatch: usize,
    /// Sections whose parts do not tile the index buffer in order (the
    /// decoder rejects these; counted here as a second line of defence).
    pub untiled_sections: usize,
    pub node_vertices: usize,
    pub bad_weight_sum: usize,
    pub materials: usize,
    /// Materials whose first reference is not null, or whose shader
    /// reference does not resolve to a `shad` tag.
    pub material_mismatch: usize,
    pub nodes: usize,
    pub vertex_kinds: BTreeMap<String, usize>,
    pub raw_files: BTreeMap<String, usize>,
    /// Sum of dot(face normal, packed vector k) over triangle corners, k = 0..3,
    /// and the number of samples. Identifies which packed vector is the normal
    /// and whether strip winding is consistent with it.
    pub frame_dot: [f64; 3],
    pub frame_samples: u64,
    /// Decoded NORMPACKED3 vectors, and those with |length - 1| > 0.05.
    pub frame_vectors: usize,
    pub non_unit: usize,
    pub foreign_node_refs: usize,
    /// Positions/UVs outside the compression bounds (tolerance 1e-4).
    pub out_of_bounds: usize,
}

/// Thresholds for counters that are known to be nonzero on the retail disc
/// (observed values in docs/formats/map-format.md). Everything else must be zero.
pub const MAX_OUT_OF_BOUNDS: usize = 16; // observed 3
pub const MAX_NON_UNIT_FRACTION: f64 = 0.005; // observed 0.0012
pub const MAX_FOREIGN_NODE_FRACTION: f64 = 0.001; // observed 0.00027
pub const MIN_NORMAL_DOT: f64 = 0.8; // observed 0.88

/// Every check that fails, as a readable line. Empty means pass.
pub fn breaches(g: &Totals) -> Vec<String> {
    let mut out = Vec::new();
    for (n, what) in [
        (g.maps_failed, "maps failed to open"),
        (g.failed, "tags failed to decode"),
        (g.triangle_mismatch, "triangle-count mismatches"),
        (g.untiled_sections, "untiled index buffers"),
        (g.bad_weight_sum, "bad weight sums"),
        (g.material_mismatch, "material reference mismatches"),
    ] {
        if n != 0 {
            out.push(format!("{n} {what} (must be 0)"));
        }
    }
    if g.out_of_bounds > MAX_OUT_OF_BOUNDS {
        out.push(format!(
            "{} out-of-bounds attributes (max {MAX_OUT_OF_BOUNDS})",
            g.out_of_bounds
        ));
    }
    let frac = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 / d as f64 };
    if frac(g.non_unit, g.frame_vectors) > MAX_NON_UNIT_FRACTION {
        out.push(format!(
            "{}/{} non-unit packed vectors (max fraction {MAX_NON_UNIT_FRACTION})",
            g.non_unit, g.frame_vectors
        ));
    }
    if frac(g.foreign_node_refs, g.node_vertices) > MAX_FOREIGN_NODE_FRACTION {
        out.push(format!(
            "{}/{} vertices reference foreign nodes (max fraction {MAX_FOREIGN_NODE_FRACTION})",
            g.foreign_node_refs, g.node_vertices
        ));
    }
    if g.frame_samples > 0 && g.frame_dot[0] / (g.frame_samples as f64) < MIN_NORMAL_DOT {
        out.push(format!(
            "mean normal/face dot {:.3} (min {MIN_NORMAL_DOT})",
            g.frame_dot[0] / g.frame_samples as f64
        ));
    }
    out
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn account(t: &mut Totals, m: &RenderModelGeometry, class_of: impl Fn(TagId) -> Option<FourCC>) {
    t.materials += m.materials.len();
    for mat in &m.materials {
        let shader_ok =
            mat.shader.id == TagId::NULL || class_of(mat.shader.id) == Some(FourCC(*b"shad"));
        t.material_mismatch += usize::from(mat.unknown_reference.id != TagId::NULL || !shader_ok);
    }
    t.nodes += m.nodes.len();
    let bounds = m.compression.as_ref();
    for s in &m.sections {
        t.sections += 1;
        *t.raw_files.entry(format!("{:?}", s.raw.file)).or_default() += 1;
        let d = &s.geometry;
        let v = &s.vertices;
        t.parts += d.parts.len();
        t.vertices += v.positions.len();
        t.indices += d.indices.len();
        for vb in &d.vertex_buffers {
            let name = match vb.kind {
                VertexKind::Unknown(b) => format!("unknown {b:#04x} stride {}", vb.stride),
                k => format!("{k:?}"),
            };
            *t.vertex_kinds.entry(name).or_default() += 1;
        }
        let mut tris = 0;
        let mut next = 0usize;
        let mut tiled = true;
        for p in &d.parts {
            tiled &= usize::from(p.first_index) == next;
            next = p.index_range().end;
            let pt = d.part_triangles(p);
            tris += pt.len();
            if v.normals.len() == v.positions.len() {
                for [a, b, c] in pt {
                    let [a, b, c] = [a, b, c].map(usize::from);
                    let n = cross(
                        sub(v.positions[b], v.positions[a]),
                        sub(v.positions[c], v.positions[a]),
                    );
                    let len = dot(n, n).sqrt();
                    if len < 1e-9 {
                        continue;
                    }
                    let n = n.map(|x| x / len);
                    for i in [a, b, c] {
                        for (k, frame) in [&v.normals, &v.binormals, &v.tangents].iter().enumerate()
                        {
                            t.frame_dot[k] += f64::from(dot(n, frame[i]));
                        }
                        t.frame_samples += 1;
                    }
                }
            }
        }
        tiled &= next == d.indices.len();
        t.untiled_sections += usize::from(!tiled);
        // Exact: the per-part topology rule must reproduce the stored count.
        t.triangle_mismatch += usize::from(tris != usize::from(s.wire.triangle_count));
        t.triangles += tris;
        for list in [&v.normals, &v.binormals, &v.tangents] {
            for n in list.iter() {
                t.frame_vectors += 1;
                t.non_unit += usize::from((dot(*n, *n).sqrt() - 1.0).abs() > 0.05);
            }
        }
        if !v.node_weights.is_empty() {
            let limit = if s.node_map.is_empty() {
                m.nodes.len()
            } else {
                s.node_map.len()
            };
            t.foreign_node_refs += v
                .node_indices
                .iter()
                .zip(&v.node_weights)
                .filter(|(n, w)| (0..4).any(|k| w[k] > 0.0 && usize::from(n[k]) >= limit))
                .count();
            t.node_vertices += v.node_weights.len();
            t.bad_weight_sum += v
                .node_weights
                .iter()
                .filter(|w| (w.iter().sum::<f32>() - 1.0).abs() > 0.02)
                .count();
        }
        if let Some(c) = bounds {
            let pb = c.position_bounds;
            let ub = c.texcoord_bounds;
            let eps = 1e-4;
            let compressed = s.wire.compression_flags.0;
            if compressed & 1 != 0 {
                t.out_of_bounds += v
                    .positions
                    .iter()
                    .filter(|p| {
                        (0..3).any(|k| p[k] < pb[2 * k] - eps || p[k] > pb[2 * k + 1] + eps)
                    })
                    .count();
            }
            if compressed & 2 != 0 {
                t.out_of_bounds += v
                    .texcoords
                    .iter()
                    .filter(|p| {
                        (0..2).any(|k| p[k] < ub[2 * k] - eps || p[k] > ub[2 * k + 1] + eps)
                    })
                    .count();
            }
        }
    }
}

/// Returns false if anything failed (see the module docs).
pub fn run(disc: &Disc, maps: &[String]) -> Result<bool> {
    let mut grand = Totals::default();
    let mut failures = Vec::new();
    for name in maps {
        let opened = disc
            .open_map(name)
            .and_then(|m| Ok((m, disc.resources(name)?)));
        let (mut map, mut raw) = match opened {
            Ok(v) => v,
            Err(e) => {
                grand.maps_failed += 1;
                failures.push(format!("{name}: cannot open: {e:#}"));
                println!("{name:<26} cannot open");
                continue;
            }
        };
        let ids: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"mode"))
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        let mut t = Totals::default();
        for (id, tag_name) in ids {
            t.tags += 1;
            match RenderModelGeometry::read(&mut map, id, &mut raw) {
                Ok(m) => account(&mut t, &m, |id| map.tag(id).ok().map(|t| t.group)),
                Err(e) => {
                    t.failed += 1;
                    failures.push(format!("{name}: {tag_name} ({id}): {e}"));
                }
            }
        }
        println!(
            "{name:<26} mode {:>4}  failed {:>3}  sections {:>5}  parts {:>6}  vertices {:>8}  triangles {:>8}",
            t.tags, t.failed, t.sections, t.parts, t.vertices, t.triangles
        );
        merge(&mut grand, t);
    }
    let g = &grand;
    println!("---");
    println!(
        "total: {} maps ({} failed to open), {} mode tags, {} failed; {} sections, {} parts, {} vertices ({} with node data), {} indices -> {} triangles; {} nodes, {} materials",
        maps.len(),
        g.maps_failed,
        g.tags,
        g.failed,
        g.sections,
        g.parts,
        g.vertices,
        g.node_vertices,
        g.indices,
        g.triangles,
        g.nodes,
        g.materials
    );
    println!(
        "checks: triangle-count mismatches {}, untiled index buffers {}, bad weight sums {}, material mismatches {}, out-of-bounds attributes {}, non-unit packed vectors {}/{}, vertices referencing nodes outside the model {}",
        g.triangle_mismatch,
        g.untiled_sections,
        g.bad_weight_sum,
        g.material_mismatch,
        g.out_of_bounds,
        g.non_unit,
        g.frame_vectors,
        g.foreign_node_refs
    );
    if g.frame_samples > 0 {
        let s = g.frame_samples as f64;
        println!(
            "mean dot(face normal, packed vector 0/1/2): {:.3} / {:.3} / {:.3}  ({} corners)",
            g.frame_dot[0] / s,
            g.frame_dot[1] / s,
            g.frame_dot[2] / s,
            g.frame_samples
        );
    }
    println!("raw block files: {:?}", g.raw_files);
    println!("vertex buffers: {:?}", g.vertex_kinds);
    for f in &failures {
        println!("FAIL {f}");
    }
    let breached = breaches(g);
    for b in &breached {
        println!("CHECK FAILED: {b}");
    }
    let ok = breached.is_empty();
    println!("{}", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

fn merge(g: &mut Totals, t: Totals) {
    g.maps_failed += t.maps_failed;
    g.tags += t.tags;
    g.failed += t.failed;
    g.sections += t.sections;
    g.parts += t.parts;
    g.vertices += t.vertices;
    g.indices += t.indices;
    g.triangles += t.triangles;
    g.triangle_mismatch += t.triangle_mismatch;
    g.untiled_sections += t.untiled_sections;
    g.node_vertices += t.node_vertices;
    g.bad_weight_sum += t.bad_weight_sum;
    g.materials += t.materials;
    g.material_mismatch += t.material_mismatch;
    g.nodes += t.nodes;
    g.out_of_bounds += t.out_of_bounds;
    g.frame_vectors += t.frame_vectors;
    g.non_unit += t.non_unit;
    g.foreign_node_refs += t.foreign_node_refs;
    for k in 0..3 {
        g.frame_dot[k] += t.frame_dot[k];
    }
    g.frame_samples += t.frame_samples;
    for (k, v) in t.vertex_kinds {
        *g.vertex_kinds.entry(k).or_default() += v;
    }
    for (k, v) in t.raw_files {
        *g.raw_files.entry(k).or_default() += v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Totals {
        Totals {
            frame_vectors: 1000,
            node_vertices: 1000,
            frame_samples: 10,
            frame_dot: [9.0, 0.0, 0.0],
            ..Default::default()
        }
    }

    #[test]
    fn clean_totals_pass() {
        assert!(breaches(&clean()).is_empty());
    }

    #[test]
    fn zero_tolerance_counters_fail_the_run() {
        type Set = fn(&mut Totals);
        let cases: [(Set, &str); 6] = [
            (|t| t.maps_failed = 1, "maps failed"),
            (|t| t.failed = 1, "failed to decode"),
            (|t| t.triangle_mismatch = 1, "triangle-count"),
            (|t| t.untiled_sections = 1, "untiled"),
            (|t| t.bad_weight_sum = 1, "weight"),
            (|t| t.material_mismatch = 1, "material"),
        ];
        for (set, word) in cases {
            let mut t = clean();
            set(&mut t);
            let b = breaches(&t);
            assert_eq!(b.len(), 1, "{word}");
            assert!(b[0].contains(word), "{b:?}");
        }
    }

    #[test]
    fn thresholded_counters_fail_only_above_their_limit() {
        let mut t = clean();
        t.out_of_bounds = MAX_OUT_OF_BOUNDS;
        t.non_unit = 5; // 0.5%
        t.foreign_node_refs = 1; // 0.1%
        assert!(breaches(&t).is_empty());
        t.out_of_bounds += 1;
        t.non_unit += 1;
        t.foreign_node_refs += 1;
        t.frame_dot[0] = 7.0;
        assert_eq!(breaches(&t).len(), 4);
    }
}
