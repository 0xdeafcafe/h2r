//! Node-local collision surfaces from the loaded map and the Havok shapes of
//! each live object's component. All emitted edges are in game world units.
use std::collections::HashMap;
use std::io::{Read, Seek};

use guest_layout::GuestMemory;
use map::{FourCC, Map, TagId};
use sim_state::NodeMatrix;
use tags::collision::{Collision, PhysicsModel};
use tags::object::{Metadata, is_object_group};

mod physics;
type Point = [f32; 3];
type Edge = [Point; 2];

// Bound the one-time node-local geometry as well as the per-frame batch.
const MAP_EDGES: usize = 512 * 1024;
const MODEL_EDGES: usize = 16 * 1024;
const MAX_NODES: usize = 256;
const MAX_REGIONS: usize = 128;

#[derive(Default)]
pub struct Hitboxes {
    definitions: HashMap<TagId, TagId>,
    models: HashMap<TagId, Model>,
}

#[derive(Default)]
struct Model {
    // Model region -> render permutation -> node-local collision surfaces.
    regions: Vec<Vec<Vec<NodeEdges>>>,
    physics: Option<(TagId, Vec<Vec<Edge>>)>,
}

struct NodeEdges {
    node: usize,
    lines: Vec<Edge>,
}

// Both descriptors consist of two little-endian words (defs/obje.txt,
// defs/hlmt.txt). Accept only a complete descriptor inside its parent record.
fn words(bytes: &[u8], at: usize) -> Option<[u32; 2]> {
    let mut words = bytes.get(at..)?.chunks_exact(4);
    Some([
        u32::from_le_bytes(words.next()?.try_into().ok()?),
        u32::from_le_bytes(words.next()?.try_into().ok()?),
    ])
}

fn reference(tag: &[u8], at: usize, group: [u8; 4]) -> Option<TagId> {
    let [class, datum] = words(tag, at)?;
    let id = TagId(datum);
    (FourCC::from_u32(class) == FourCC(group) && id != TagId::NULL).then_some(id)
}

fn block<'a>(
    meta: &'a Metadata,
    tag: &[u8],
    at: usize,
    stride: usize,
    cap: usize,
) -> Option<std::slice::ChunksExact<'a, u8>> {
    let [count, address] = words(tag, at)?;
    if count == 0 {
        return Some([].chunks_exact(stride));
    }
    let n = usize::try_from(count).ok()?;
    if n > cap {
        return None;
    }
    Some(
        meta.slice(address, (n * stride) as u64)
            .ok()?
            .chunks_exact(stride),
    )
}

impl Hitboxes {
    /// Build node-local collision and shape outlines from this map's metadata.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> tags::Result<Self> {
        let meta = Metadata::read(map)?;
        let mut result = Self::default();
        let mut remaining = MAP_EDGES;
        for tag in map.tags() {
            if !is_object_group(tag.group) {
                continue;
            }
            let Some(loc) = tag.location else { continue };
            let Ok(object) = meta.slice(loc.address, loc.size.into()) else {
                continue;
            };
            // defs/obje.txt, the model reference shared by all object groups.
            if let Some(id) = reference(object, 0x34, *b"hlmt") {
                result.definitions.insert(tag.id, id);
            }
        }
        // Map order also defines the deterministic cache budget.
        let referenced: std::collections::HashSet<_> =
            result.definitions.values().copied().collect();
        let models: Vec<_> = map
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"hlmt") && referenced.contains(&t.id))
            .filter_map(|t| Some((t.id, t.location?)))
            .collect();
        for (id, loc) in models {
            if remaining == 0 {
                break;
            }
            let Ok(hlmt) = meta.slice(loc.address, loc.size.into()) else {
                continue;
            };
            let mut budget = remaining.min(MODEL_EDGES);
            let start = budget;
            let regions = reference(hlmt, 8, *b"coll")
                .and_then(|id| Collision::read(map, id).ok())
                .and_then(|c| collision_regions(&meta, hlmt, &c, &mut budget))
                .unwrap_or_default();
            let physics = reference(hlmt, 0x20, *b"phmo").and_then(|id| {
                let phmo = PhysicsModel::read(map, id).ok()?;
                Some((id, physics::body_edges(&phmo, &mut budget)))
            });
            remaining -= start - budget;
            result.models.insert(id, Model { regions, physics });
        }
        Ok(result)
    }

    pub(super) fn edges(
        &self,
        guest: &dyn GuestMemory,
        cameras: &[Point],
        range: f32,
        max_lines: usize,
    ) -> Vec<Edge> {
        let mut lines = Vec::new();
        let Ok(objects) = sim_state::objects(guest) else {
            return lines;
        };
        let mut shape_pointers = HashMap::new();
        for object in objects.flatten() {
            if lines.len() == max_lines {
                break;
            }
            if !cameras.is_empty()
                && let Ok(bounds) = object.bounding_sphere()
                && bounds.radius >= 0.0
                && bounds.center.iter().all(|n| n.is_finite())
                && !cameras.iter().any(|cam| {
                    let d = std::array::from_fn(|i| bounds.center[i] - cam[i]);
                    dot(d, d) <= (range + bounds.radius).powi(2)
                })
            {
                continue;
            }
            let model = object
                .definition()
                .ok()
                .and_then(|d| self.definitions.get(&TagId(d.raw())))
                .and_then(|id| self.models.get(id));
            if let Some(model) = model
                && !model.regions.is_empty()
            {
                let mut nodes = [NodeMatrix::default(); MAX_NODES];
                let mut perms = [0u8; MAX_REGIONS];
                if let (Ok(n), Ok(r)) = (
                    object.nodes(&mut nodes),
                    object.region_permutations(&mut perms[..model.regions.len()]),
                ) {
                    for (choices, &selection) in model.regions.iter().zip(&perms[..r]) {
                        let Some(parts) = choices.get(usize::from(selection)) else {
                            continue;
                        };
                        for part in parts {
                            if let Some(pose) = nodes[..n.min(MAX_NODES)].get(part.node) {
                                append(&mut lines, max_lines, pose, &part.lines);
                            }
                        }
                    }
                }
            }
            if lines.len() < max_lines {
                physics::live_edges(
                    guest,
                    object.id().0,
                    model.and_then(|m| m.physics.as_ref()),
                    &mut shape_pointers,
                    &mut lines,
                    max_lines,
                );
            }
        }
        lines
    }
}

fn dot(a: Point, b: Point) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn point(m: &NodeMatrix, p: Point) -> Point {
    let v = &m.0;
    std::array::from_fn(|i| {
        v[10 + i] + v[0] * (v[1 + i] * p[0] + v[4 + i] * p[1] + v[7 + i] * p[2])
    })
}

fn append(out: &mut Vec<Edge>, cap: usize, matrix: &NodeMatrix, edges: &[Edge]) {
    if matrix.0[0] == 0.0 || !matrix.0.iter().all(|n| n.is_finite()) {
        return;
    }
    for edge in edges.iter().take(cap.saturating_sub(out.len())) {
        let world = edge.map(|v| point(matrix, v));
        if world.iter().flatten().all(|n| n.is_finite()) {
            out.push(world);
        }
    }
}

fn collision_regions(
    meta: &Metadata,
    hlmt: &[u8],
    coll: &Collision,
    budget: &mut usize,
) -> Option<Vec<Vec<Vec<NodeEdges>>>> {
    // defs/hlmt.txt: render pose nodes are named. A coll BSP's leading
    // word indexes coll.nodes (docs/formats/collision.md, "Not established").
    let nodes: HashMap<_, _> = block(meta, hlmt, 0x78, 0x5c, MAX_NODES)?
        .enumerate()
        .map(|(i, n)| (u32::from_le_bytes(n[..4].try_into().unwrap()), i))
        .collect();
    let mut result = Vec::new();
    for region in block(meta, hlmt, 0x70, 0x10, MAX_REGIONS)? {
        let r = coll
            .regions
            .get(region[4] as usize)
            .filter(|_| region[4] < 128);
        let mut choices = Vec::new();
        for perm in block(meta, region, 8, 8, 128)? {
            let mut parts = Vec::new();
            if let Some(p) = r
                .and_then(|r| r.permutations.get(perm[5] as usize))
                .filter(|_| perm[5] < 128)
            {
                for bsp in &p.bsps {
                    if *budget == 0 {
                        break;
                    }
                    let Some(node) = coll
                        .nodes
                        .get(bsp.unknown_00 as usize)
                        .and_then(|n| nodes.get(&n.name.0))
                    else {
                        continue;
                    };
                    let lines: Vec<_> = bsp
                        .edges
                        .iter()
                        .filter_map(|edge| {
                            let a = bsp.vertices.get(usize::from(edge.start_vertex))?.point;
                            let b = bsp.vertices.get(usize::from(edge.end_vertex))?.point;
                            [a, b]
                                .iter()
                                .flatten()
                                .all(|n| n.is_finite())
                                .then_some([a, b])
                        })
                        .take(*budget)
                        .collect();
                    *budget -= lines.len();
                    if !lines.is_empty() {
                        parts.push(NodeEdges { node: *node, lines });
                    }
                }
            }
            choices.push(parts);
        }
        result.push(choices);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{testing::FakeGuest, view::{fake, objects}};
    use guest_layout::{Datum, Object, Off};

    #[test]
    fn model_regions_bind_collision_edges_by_name_and_permutation() {
        use tags::collision::{Bsp, CollNode, Edge as BspEdge, Permutation, Region, Vertex};
        use tags::wire::StringId;

        let mut bytes = vec![0; 0x200];
        let base = 4096;
        let descriptor = |b: &mut [u8], at: usize, count: u32, offset: u32| {
            b[at..at + 4].copy_from_slice(&count.to_le_bytes());
            b[at + 4..at + 8].copy_from_slice(&(base + offset).to_le_bytes());
        };
        descriptor(&mut bytes, 0x78, 2, 0x80);
        bytes[0xdc..0xe0].copy_from_slice(&37u32.to_le_bytes()); // the second pose node
        descriptor(&mut bytes, 0x70, 1, 0x140);
        descriptor(&mut bytes, 0x148, 2, 0x160);
        bytes[0x165] = 255; // the first rendered permutation has no collision
        let meta = Metadata { base, bytes: std::sync::Arc::new(bytes) };
        let edges = vec![BspEdge {
            start_vertex: 0,
            end_vertex: 1,
            forward_edge: 0,
            reverse_edge: 0,
            left_surface: 0,
            right_surface: u16::MAX,
        }];
        let vertices = vec![
            Vertex {
                point: [0., 0., 0.],
                edge: 0,
            },
            Vertex {
                point: [1., 2., 3.],
                edge: 0,
            },
        ];
        let bsp = Bsp {
            vertices,
            edges,
            ..Bsp::default()
        };
        let perm = Permutation {
            name: StringId(1),
            bsps: vec![bsp],
            bsp_physics_count: 0,
            bsp_physics: vec![],
        };
        let c = Collision {
            reserved: [0; 5],
            materials: vec![],
            pathfinding_spheres: vec![],
            nodes: vec![CollNode {
                name: StringId(37),
                region: 0,
                parent: -1,
                next_sibling: -1,
                first_child: -1,
            }],
            regions: vec![Region {
                name: StringId(2),
                permutations: vec![perm],
            }],
        };
        let mut budget = 1;
        let regions = collision_regions(&meta, &meta.bytes, &c, &mut budget).unwrap();
        assert_eq!(budget, 0);
        assert!(regions[0][0].is_empty());
        assert_eq!(regions[0][1][0].node, 1);
        assert_eq!(regions[0][1][0].lines, vec![[[0., 0., 0.], [1., 2., 3.]]]);
    }

    #[test]
    fn hitboxes_follow_the_selected_permutation_and_its_world_node_each_tick() {
        const BODY: u32 = 0x9000;
        let mut guest = FakeGuest::new();
        guest.put_u32(guest_layout::points::OBJECTS.base, 0x8000);
        let pool = fake::datums::<objects::Header>(&mut guest, 0x8000, 4);
        let (_, header) = fake::push(&mut guest, &pool);
        header.set_body(&mut guest, BODY).unwrap();
        guest.map(BODY, 0x800);
        let body = Object::at(BODY);
        body.set(&mut guest, Object::DEFINITION, Datum(10)).unwrap();
        body.set(&mut guest, Object::BOUND_CENTER, [5., 5., 5.])
            .unwrap();
        body.set(&mut guest, Object::BOUND_RADIUS, 4.).unwrap();
        body.set(&mut guest, Object::PHYSICS, u32::MAX).unwrap();
        body.set(&mut guest, Object::REGIONS_OFFSET, 0x400).unwrap();
        body.set(&mut guest, Object::REGIONS_SIZE, 10).unwrap();
        body.set(&mut guest, Object::NODES_OFFSET, 0x500).unwrap();
        body.set(&mut guest, Object::NODES_SIZE, 2 * 0x34).unwrap();
        // Only the second node carries collision; it is rotated and scaled.
        let node = Off::<[f32; 13]>::new(0);
        let mut pose = [2., 0., 1., 0., -1., 0., 0., 0., 0., 1., 5., 5., 5.];
        node.write(&mut guest, BODY + 0x534, pose).unwrap();
        let edges = vec![[[1., 0., 0.], [0., 1., 0.]], [[0., 1., 0.], [0., 0., 1.]]];
        let data = Hitboxes {
            definitions: HashMap::from([(TagId(10), TagId(20))]),
            models: HashMap::from([(
                TagId(20),
                Model {
                    regions: vec![vec![
                        vec![],
                        vec![NodeEdges {
                            node: 1,
                            lines: edges,
                        }],
                    ]],
                    physics: None,
                },
            )]),
        };
        assert!(data.edges(&guest, &[], 10., 4).is_empty());
        guest.put_u8(BODY + 0x400, 1);
        assert_eq!(
            data.edges(&guest, &[[5.; 3]], 2., 1),
            vec![[[5., 7., 5.], [3., 5., 5.]]]
        );
        assert!(data.edges(&guest, &[[80.; 3]], 2., 1).is_empty());
        pose[12] += 1.;
        node.write(&mut guest, BODY + 0x534, pose).unwrap();
        assert_eq!(data.edges(&guest, &[], 10., 4)[0][0], [5., 7., 6.]);
        guest.put_u8(BODY + 0x400, 255);
        assert!(data.edges(&guest, &[], 10., 4).is_empty());
    }
}
