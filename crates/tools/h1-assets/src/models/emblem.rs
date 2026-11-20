//! The player's emblem on a Halo 1 player biped's model.
//!
//! Halo 2 draws a player's emblem as a part of its own on the biped's
//! render model: the Chief's `mc_emblem` material
//! (`shaders\shader_templates\opaque\emblem_overlay_simple`) samples the
//! engine's emblem bitmap (`ui\global_bitmaps\emblems\foreground`, its
//! frame and colours animated from the inputs `emblem_foreground_index`,
//! `emblem_toggle_foreground` and `emblem_active`, which the engine answers
//! for the drawn player at `0x0022ace7`) with its border clamped
//! (sampler address 4), so the part's texture coordinates 0 to 1 are the
//! emblem and nothing of it shows outside them. Halo 1's cyborg has no
//! such part: Halo 1 had no emblems on bodies.
//!
//! So the converted model gets one ([`add_emblem`]): a square of side
//! [`Placement::size`] lying on the model's surface where a ray from the
//! anchor node meets it ([`place`]), lifted a little off it along the
//! surface normal, its texture coordinates 0 to 1, every vertex bound to
//! the anchor node alone, drawn with a shader the converter names (the
//! base's `mc_emblem`). It goes into each detail level's geometry of the
//! permutation it was placed on. Design and evidence:
//! `docs/halo1/models.md`, "The emblem".

use crate::models::h1::{Model, Part, ShaderRef, Vertex, strip_triangles};
use crate::models::mode::world_transforms;

/// Where the emblem goes: a ray in the model's default pose from the
/// anchor node, `along` its own x axis (a bone's length direction), cast
/// in the model-space direction `direction`; the outermost surface of
/// the triangles bound to the anchor that it meets within `reach` is the
/// spot. `up` (model space) is the emblem's up, laid onto the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub node: &'static str,
    pub along: f32,
    pub direction: [f32; 3],
    pub up: [f32; 3],
    pub reach: f32,
    /// Side of the square, world units.
    pub size: f32,
    /// How far off the surface it lies, world units.
    pub lift: f32,
}

/// The default: the outside of the cyborg's left shoulder plate. Halo 1's
/// models face +x with +z up, so +y is the biped's left; the upper arm
/// hangs down its own x axis from the shoulder joint (V: Blood Gulch's
/// `characters\cyborg\cyborg`, the joint at (-0.008, 0.09, 0.569), the
/// arm's x axis (0.01, 0.19, -0.98)), and the plate covers its first
/// 0.03 to 0.06. 0.07 units (about 21 cm) is most of the plate's width.
pub const LEFT_SHOULDER: Placement = Placement {
    node: "bip01 l upperarm",
    along: 0.03,
    direction: [0.0, 1.0, 0.0],
    up: [0.0, 0.0, 1.0],
    reach: 0.3,
    size: 0.07,
    lift: 0.002,
};

/// The group the emblem's shader entry carries: not a Halo 1 shader
/// group, so the part draws as Halo 2's transparent kind (the converter
/// writes kind 2 only for `soso`), after the opaque body it lies on.
pub const EMBLEM_GROUP: h1map::Group = h1map::Group::new(b"embl");

/// A spot found by [`place`], model space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub node: i16,
    /// The permutation (region, permutation) whose geometry was hit.
    pub region: usize,
    pub permutation: usize,
    pub centre: [f32; 3],
    pub normal: [f32; 3],
    pub up: [f32; 3],
    pub right: [f32; 3],
    /// Whether the model's triangles face where their geometric normal
    /// `(b - a) x (c - a)` points (as the hit triangle's vertex normals
    /// say), so the square is wound the same way.
    pub front_is_geometric: bool,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
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
fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-6 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

/// Where the ray from `o` along `d` meets triangle `abc`, as its distance
/// (Moller-Trumbore); `None` when it misses or the triangle is edge on.
fn hit(o: [f32; 3], d: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let (e1, e2) = (sub(b, a), sub(c, a));
    let p = cross(d, e2);
    let det = dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = sub(o, a);
    let u = dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(e2, q) * inv;
    (t > 0.0).then_some(t)
}

/// Whether vertex `v` is bound mostly to `node`.
fn bound_to(v: &Vertex, node: i16) -> bool {
    v.nodes[0] == node && (v.nodes[1] < 0 || v.nodes[1] == node || v.weights[0] >= 0.5)
}

/// The spot for `p` on `model`: the farthest hit (the outer surface) of
/// the ray on a triangle whose three vertices are bound to the anchor, in
/// the most detailed geometry of each permutation in turn (the first
/// permutation with a hit wins). `None` without the anchor node or a hit.
pub fn place(model: &Model, p: &Placement) -> Option<Spot> {
    let node = model
        .nodes
        .iter()
        .position(|n| n.name.eq_ignore_ascii_case(p.node))?;
    let pose: Vec<_> = model
        .nodes
        .iter()
        .map(|n| (n.parent, n.rotation, n.translation))
        .collect();
    let (m, at) = world_transforms(&pose)[node];
    let x_axis = [m[0][0], m[1][0], m[2][0]];
    let origin = add(at, scale(x_axis, p.along));
    let d = unit(p.direction)?;
    let node = node as i16;
    for (ri, r) in model.regions.iter().enumerate() {
        for (pi, perm) in r.permutations.iter().enumerate() {
            let Some(g) = usize::try_from(perm.lods[4])
                .ok()
                .and_then(|g| model.geometries.get(g))
            else {
                continue;
            };
            // (distance, triangle normal, its vertex normals' sum)
            let mut best: Option<(f32, [f32; 3], [f32; 3])> = None;
            for part in &g.parts {
                for t in strip_triangles(&part.strip) {
                    let vs: Option<Vec<&Vertex>> = t
                        .iter()
                        .map(|&i| part.vertices.get(usize::from(i)))
                        .collect();
                    let Some(vs) = vs else { continue };
                    if !vs.iter().all(|v| bound_to(v, node)) {
                        continue;
                    }
                    let (a, b, c) = (vs[0].position, vs[1].position, vs[2].position);
                    let Some(dist) = hit(origin, d, a, b, c) else {
                        continue;
                    };
                    if dist > p.reach || best.is_some_and(|x| x.0 >= dist) {
                        continue;
                    }
                    let geometric = cross(sub(b, a), sub(c, a));
                    let normals = vs.iter().fold([0.0; 3], |s, v| add(s, v.normal));
                    best = Some((dist, geometric, normals));
                }
            }
            let Some((dist, geometric, normals)) = best else {
                continue;
            };
            let front_is_geometric = dot(geometric, normals) >= 0.0;
            // The surface's outward normal: the side the ray left by.
            let mut normal = unit(geometric)?;
            if dot(normal, d) < 0.0 {
                normal = scale(normal, -1.0);
            }
            let up = unit(sub(p.up, scale(normal, dot(p.up, normal))))
                .or_else(|| unit(cross(normal, [1.0, 0.0, 0.0])))?;
            let right = cross(up, normal);
            let centre = add(add(origin, scale(d, dist)), scale(normal, p.lift));
            return Some(Spot {
                node,
                region: ri,
                permutation: pi,
                centre,
                normal,
                up,
                right,
                front_is_geometric,
            });
        }
    }
    None
}

/// The emblem's part at `spot`: four vertices (top left, top right,
/// bottom left, bottom right; texture coordinates 0 to 1 across and down,
/// divided by the model's base map scale, which the converter multiplies
/// back), every one bound to the anchor alone, in a strip whose two
/// triangles face out the way the model's do; `shader` is its index.
pub fn part(model: &Model, spot: &Spot, size: f32, shader: i16) -> Part {
    let s = model.base_map_scale;
    let inv = |x: f32| if x == 0.0 { 1.0 } else { 1.0 / x };
    let corner = |u: f32, v: f32| Vertex {
        position: add(
            spot.centre,
            add(
                scale(spot.right, (u - 0.5) * size),
                scale(spot.up, (0.5 - v) * size),
            ),
        ),
        normal: spot.normal,
        binormal: scale(spot.up, -1.0),
        tangent: spot.right,
        texcoord: [u * inv(s[0]), v * inv(s[1])],
        nodes: [spot.node, -1],
        weights: [1.0, 0.0],
    };
    let (tl, tr, bl, br) = (
        corner(0.0, 0.0),
        corner(1.0, 0.0),
        corner(0.0, 1.0),
        corner(1.0, 1.0),
    );
    // With strip_triangles' parity, the strip TL, TR, BL, BR gives
    // (TL, BL, TR) and (TR, BL, BR), whose geometric normal is the
    // spot's; swapping TR and BL turns both.
    let vertices = if spot.front_is_geometric {
        vec![tl, tr, bl, br]
    } else {
        vec![tl, bl, tr, br]
    };
    Part {
        flags: 0,
        shader,
        centroid_nodes: [spot.node, -1],
        centroid_weights: [1.0, 0.0],
        centroid: spot.centre,
        vertices,
        strip: vec![0, 1, 2, 3],
    }
}

/// Adds the emblem to `model` at `p`: a shader entry of group
/// [`EMBLEM_GROUP`] (no Halo 1 tag; the converter names Halo 2's emblem
/// shader for it) and the emblem's part in every geometry the placed
/// permutation draws at any detail level. Returns the shader entry's
/// index, or `None` (the model unchanged) when [`place`] finds no spot.
pub fn add_emblem(model: &mut Model, p: &Placement) -> Option<usize> {
    let spot = place(model, p)?;
    let index = model.shaders.len();
    let shader = i16::try_from(index).ok()?;
    let part = part(model, &spot, p.size, shader);
    model.shaders.push(ShaderRef {
        group: EMBLEM_GROUP,
        tag: h1map::TagId::NONE,
        permutation: 0,
    });
    let mut geometries: Vec<usize> = model.regions[spot.region].permutations[spot.permutation]
        .lods
        .iter()
        .filter_map(|&g| usize::try_from(g).ok())
        .filter(|&g| g < model.geometries.len())
        .collect();
    geometries.sort_unstable();
    geometries.dedup();
    for g in geometries {
        model.geometries[g].parts.push(part.clone());
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::h1::{Geometry, Node, Permutation, Region};

    const LIFT: f32 = 0.002;

    fn placement() -> Placement {
        Placement {
            lift: LIFT,
            ..LEFT_SHOULDER
        }
    }

    fn node(name: &str, parent: i16, translation: [f32; 3]) -> Node {
        Node {
            name: name.into(),
            next_sibling: -1,
            first_child: -1,
            parent,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            distance_from_parent: 0.0,
        }
    }

    /// A square of side 0.2 at `centre` facing `normal` (vertex normals
    /// too), bound to `node`; `geometric` says whether its triangles'
    /// `(b - a) x (c - a)` points the same way.
    fn face(m: &Model, centre: [f32; 3], normal: [f32; 3], node: i16, geometric: bool) -> Part {
        let up = if normal[2].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let spot = Spot {
            node,
            region: 0,
            permutation: 0,
            centre,
            normal,
            up,
            right: cross(up, normal),
            front_is_geometric: geometric,
        };
        part(m, &spot, 0.2, 0)
    }

    /// A root and the left upper arm 0.5 above it; geometry 0 (every
    /// detail level but super low) is the arm: a box side at y 0.05 and
    /// y -0.05, a plate further out at y 0.08, and, further still, a
    /// square bound to the root that is not the arm's; geometry 1 (super
    /// low) is empty; geometry 2 belongs to no permutation.
    fn model(geometric: bool) -> Model {
        let mut m = Model {
            flags: 0,
            node_list_checksum: 0,
            lod_cutoffs: [0.0; 5],
            base_map_scale: [2.0, 4.0],
            markers: Vec::new(),
            nodes: vec![
                node("bip01 pelvis", -1, [0.0; 3]),
                node("bip01 l upperarm", 0, [0.0, 0.0, 0.5]),
            ],
            regions: vec![Region {
                name: "cyborg".into(),
                permutations: vec![Permutation {
                    name: "__base".into(),
                    flags: 0,
                    lods: [1, 0, 0, 0, 0],
                }],
            }],
            geometries: Vec::new(),
            shaders: vec![ShaderRef {
                group: h1map::Group::new(b"soso"),
                tag: h1map::TagId(7),
                permutation: 0,
            }],
        };
        let parts = vec![
            face(&m, [0.03, 0.05, 0.5], [0.0, 1.0, 0.0], 1, geometric),
            face(&m, [0.03, -0.05, 0.5], [0.0, -1.0, 0.0], 1, geometric),
            face(&m, [0.03, 0.08, 0.5], [0.0, 1.0, 0.0], 1, geometric),
            face(&m, [0.03, 0.12, 0.5], [0.0, 1.0, 0.0], 0, geometric),
        ];
        m.geometries = vec![
            Geometry { flags: 0, parts },
            Geometry {
                flags: 0,
                parts: Vec::new(),
            },
            Geometry {
                flags: 0,
                parts: Vec::new(),
            },
        ];
        m
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    #[test]
    fn the_emblem_sits_on_the_outermost_surface_of_the_anchor_facing_out() {
        let spot = place(&model(true), &placement()).expect("a spot");
        assert_eq!((spot.node, spot.region, spot.permutation), (1, 0, 0));
        // The ray starts 0.03 down the arm's x axis and leaves along +y;
        // the plate at 0.08 is the farthest arm surface, the root's square
        // at 0.12 is not the arm's.
        assert!(
            close(spot.centre, [0.03, 0.08 + LIFT, 0.5]),
            "{:?}",
            spot.centre
        );
        assert!(close(spot.normal, [0.0, 1.0, 0.0]), "{:?}", spot.normal);
        assert!(close(spot.up, [0.0, 0.0, 1.0]), "{:?}", spot.up);
        // Seen from outside (+y, looking at -y, z up), right is -x.
        assert!(close(spot.right, [-1.0, 0.0, 0.0]), "{:?}", spot.right);
        assert!(spot.front_is_geometric);
    }

    #[test]
    fn nothing_is_placed_without_the_anchor_or_a_surface_in_reach() {
        let mut m = model(true);
        m.nodes[1].name = "bip01 r upperarm".into();
        let before = m.clone();
        assert_eq!(add_emblem(&mut m, &placement()), None);
        assert_eq!(m, before);
        let mut m = model(true);
        let short = Placement {
            reach: 0.04,
            ..placement()
        };
        assert_eq!(place(&m, &short), None);
        assert_eq!(add_emblem(&mut m, &short), None);
    }

    /// Each triangle's `(b - a) x (c - a)`, as the converter will wind it.
    fn geometric_normals(p: &Part) -> Vec<[f32; 3]> {
        strip_triangles(&p.strip)
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| p.vertices[usize::from(i)].position);
                unit(cross(sub(b, a), sub(c, a))).unwrap()
            })
            .collect()
    }

    #[test]
    fn the_emblem_is_a_unit_square_of_its_own_shader_in_every_detail_level() {
        let mut m = model(true);
        let before = m.clone();
        let index = add_emblem(&mut m, &placement()).expect("placed");
        assert_eq!(index, 1);
        assert_eq!(&m.shaders[..1], &before.shaders[..]);
        assert_eq!(
            m.shaders[1],
            ShaderRef {
                group: EMBLEM_GROUP,
                tag: h1map::TagId::NONE,
                permutation: 0
            }
        );
        // Geometries 0 and 1 are the permutation's; 2 is nobody's.
        for g in 0..2 {
            assert_eq!(
                m.geometries[g].parts.len(),
                before.geometries[g].parts.len() + 1
            );
            assert_eq!(
                &m.geometries[g].parts[..before.geometries[g].parts.len()],
                &before.geometries[g].parts[..]
            );
        }
        assert_eq!(m.geometries[2], before.geometries[2]);
        let p = m.geometries[1].parts[0].clone();
        assert_eq!(m.geometries[0].parts.last(), Some(&p));
        assert_eq!(p.shader, 1);
        assert_eq!(p.strip, [0, 1, 2, 3]);
        assert_eq!(p.vertices.len(), 4);
        let s = 0.07 / 2.0;
        let c = [0.03, 0.08 + LIFT, 0.5];
        // Top left is +x (the viewer's left) and +z; texture coordinates
        // come back to 0 to 1 once the converter multiplies by (2, 4).
        let want = [
            ([c[0] + s, c[1], c[2] + s], [0.0, 0.0]),
            ([c[0] - s, c[1], c[2] + s], [0.5, 0.0]),
            ([c[0] + s, c[1], c[2] - s], [0.0, 0.25]),
            ([c[0] - s, c[1], c[2] - s], [0.5, 0.25]),
        ];
        for (v, (at, uv)) in p.vertices.iter().zip(want) {
            assert!(close(v.position, at), "{:?} vs {at:?}", v.position);
            assert_eq!(v.texcoord, uv);
            assert!(close(v.normal, [0.0, 1.0, 0.0]));
            assert!(close(v.tangent, [-1.0, 0.0, 0.0]));
            assert!(close(v.binormal, [0.0, 0.0, -1.0]));
            assert_eq!((v.nodes, v.weights), ([1, -1], [1.0, 0.0]));
        }
        assert_eq!(
            (p.centroid_nodes, p.centroid_weights),
            ([1, -1], [1.0, 0.0])
        );
        let normals = geometric_normals(&p);
        assert_eq!(normals.len(), 2);
        assert!(
            normals.iter().all(|n| close(*n, [0.0, 1.0, 0.0])),
            "{normals:?}"
        );
    }

    #[test]
    fn the_emblem_is_wound_like_the_model_it_lies_on() {
        let mut m = model(false);
        let spot = place(&m, &placement()).expect("a spot");
        assert!(!spot.front_is_geometric);
        assert!(close(spot.normal, [0.0, 1.0, 0.0]));
        add_emblem(&mut m, &placement()).expect("placed");
        let p = m.geometries[0].parts.last().unwrap();
        // The model's own triangles face against their geometric normal;
        // so do the emblem's, and the same texture corners stay put.
        let normals = geometric_normals(p);
        assert!(
            normals.iter().all(|n| close(*n, [0.0, -1.0, 0.0])),
            "{normals:?}"
        );
        let corners: Vec<_> = p.vertices.iter().map(|v| v.texcoord).collect();
        assert_eq!(corners, [[0.0, 0.0], [0.0, 0.25], [0.5, 0.0], [0.5, 0.25]]);
    }
}
