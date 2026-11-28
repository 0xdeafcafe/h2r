//! The `bloc`, the `hlmt` and the model the `mode` writer takes.

use anyhow::{Context, Result, anyhow, ensure};
use h1_assets::models::h1 as src;
use h1_assets::models::mode::hlmt_nodes;
use h1objects::rebuild;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Val};

use crate::blockout::mesh::Mesh;



/// The model the `mode` writer takes: one node at the origin, one region
/// and permutation, one geometry of one part a paint, every vertex rigid
/// on the node. With no mesh, the node alone (for the `hlmt`'s nodes).
pub fn model(mesh: Option<&Mesh>) -> Result<src::Model> {
    let mut geometries = Vec::new();
    let mut shaders = Vec::new();
    if let Some(mesh) = mesh {
        let mut parts = Vec::with_capacity(mesh.parts.len());
        let mut total = 0usize;
        for (k, part) in mesh.parts.iter().enumerate() {
            total += part.vertices.len();
            ensure!(
                total <= usize::from(u16::MAX),
                "the render mesh has more than 65535 vertices"
            );
            let n = part.vertices.len().max(1) as f32;
            let mut centroid = [0.0f32; 3];
            for v in &part.vertices {
                for (c, p) in centroid.iter_mut().zip(v.position) {
                    *c += p / n;
                }
            }
            parts.push(src::Part {
                flags: 0,
                shader: k as i16,
                centroid_nodes: [0, -1],
                centroid_weights: [1.0, 0.0],
                centroid,
                vertices: part
                    .vertices
                    .iter()
                    .map(|v| src::Vertex {
                        position: v.position,
                        normal: v.normal,
                        binormal: v.binormal,
                        tangent: v.tangent,
                        texcoord: v.uv,
                        nodes: [0, -1],
                        weights: [1.0, 0.0],
                    })
                    .collect(),
                strip: src::triangles_strip(&part.triangles),
            });
            // An opaque part: the writer draws a `soso` part opaque.
            shaders.push(src::ShaderRef {
                group: h1map::Group::new(b"soso"),
                tag: h1map::TagId(0),
                permutation: 0,
            });
        }
        geometries.push(src::Geometry { flags: 0, parts });
    }
    Ok(src::Model {
        flags: 0,
        node_list_checksum: 0,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0, 1.0],
        markers: Vec::new(),
        nodes: vec![src::Node {
            name: crate::blockout::data::data().node.as_str().to_owned(),
            next_sibling: -1,
            first_child: -1,
            parent: -1,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            distance_from_parent: 0.0,
        }],
        regions: vec![src::Region {
            name: crate::blockout::data::data().region.as_str().to_owned(),
            permutations: vec![src::Permutation {
                name: crate::blockout::data::data().region.as_str().to_owned(),
                flags: 0,
                lods: [0; 5],
            }],
        }],
        geometries,
        shaders,
    })
}

/// The `hlmt`: the render model (none for a barrier), the collision and
/// the physics model, one region whose permutation names collision and
/// physics permutation 0, the node, and the collision material mapped to
/// the global one.
pub fn hlmt(
    render: Option<TagId>,
    collision: TagId,
    physics: TagId,
    material: rebuild::ModelMaterial,
) -> Result<Block> {
    let nodes = hlmt_nodes(&model(None)?);
    rebuild::model(&rebuild::Model {
        render: render.unwrap_or(TagId::NULL),
        collision: Some(collision),
        animation: None,
        physics: Some(physics),
        nodes,
        regions: vec![rebuild::Region {
            name: crate::blockout::data::data().region.as_str().to_owned(),
            collision: 0,
            physics: 0,
            permutations: vec![(crate::blockout::data::data().region.as_str().to_owned(), 0, 0)],
        }],
        materials: vec![material],
    })
    .map_err(|e| anyhow!("hlmt: {e}"))
}

/// The `bloc`: the object header with the bounding sphere of the box from
/// `lo` to `hi` and the model, everything else at its definition's default.
pub fn bloc(hlmt: TagId, lo: [f32; 3], hi: [f32; 3]) -> Result<Block> {
    let d = Defs::builtin()
        .group(FourCC(*b"bloc"))
        .context("no definition for bloc")?;
    let mut r = defs::default_record(d);
    let centre: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
    let half: [f32; 3] = std::array::from_fn(|k| (hi[k] - lo[k]) * 0.5);
    let radius = (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt();
    let mut set = |path: &str, v: Val| r.set(path, v).map_err(|e| anyhow!("bloc {path}: {e}"));
    set("object.object_type", Val::Enum(crate::blockout::data::data().crate_type))?;
    set("object.bounding_radius", Val::Float(radius))?;
    set("object.bounding_offset", Val::floats(&centre))?;
    set("object.model", Val::tag(FourCC(*b"hlmt"), hlmt))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|e| anyhow!("bloc: {e}"))
}
