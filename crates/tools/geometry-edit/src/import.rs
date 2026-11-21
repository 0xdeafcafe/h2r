//! A decoded section as the kernel's `Source`: every vertex with its
//! attributes, every part's triangles as one face group.

use mesh_edit::{Corner, Skin, Source};
use tags::geometry::{Compression, Section};

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 && l.is_finite() {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0; 3]
    }
}

/// The skin of a vertex's node bytes and weights, when they sum to 255
/// (the kernel's invariant); `None` otherwise.
fn skin_of(nodes: [u8; 4], weights: [f32; 4]) -> Option<Skin> {
    let s = Skin {
        nodes,
        weights: weights.map(|w| (w * 255.0).round().clamp(0.0, 255.0) as u8),
    };
    (s.total() == 255).then_some(s)
}

/// The section's vertices and triangles. Normals are made unit (the
/// packed 11:11:10 words are near unit), so the kernel's invariants hold;
/// a clean corner still writes back its source bytes. Part `i` is face
/// group `i`, its material the faces' material; strips are expanded with
/// their degenerate joins dropped.
pub(crate) fn section_source(
    s: &Section,
    compression: Option<&Compression>,
) -> tags::Result<Source> {
    let v = s.vertices(compression)?;
    let n = s.vertex_count();
    let three = |list: &[[f32; 3]], i: usize| list.get(i).copied().unwrap_or([0.0; 3]);
    let corners = (0..n)
        .map(|i| Corner {
            uv: v.texcoords.get(i).copied().unwrap_or([0.0; 2]),
            lightmap: v.lightmap_texcoords.get(i).copied().unwrap_or([0.0; 2]),
            normal: unit(three(&v.normals, i)),
            binormal: three(&v.binormals, i),
            tangent: three(&v.tangents, i),
            light_dir: [0.0; 3],
            src: None,
            dirty: 0,
        })
        .collect();
    let skins = if v.node_indices.len() == n && v.node_weights.len() == n {
        (0..n)
            .map(|i| skin_of(v.node_indices[i], v.node_weights[i]))
            .collect()
    } else {
        Vec::new()
    };
    let mut triangles = Vec::new();
    let mut groups = Vec::new();
    let mut materials = Vec::new();
    for (pi, p) in s.parts.iter().enumerate() {
        for t in s.part_triangles(p) {
            triangles.push(t.map(u32::from));
            groups.push(pi as u16);
            materials.push(p.material);
        }
    }
    Ok(Source {
        positions: v.positions,
        corners,
        skins,
        triangles,
        groups,
        materials,
    })
}
