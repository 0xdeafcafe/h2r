//! Each placed object's collision as wire shapes (docs/editor/polish-plan.md,
//! slice P11): the `coll` BSPs and the `phmo` shapes its model names
//! (object → `hlmt` `+0x8` and `+0x20`, `tags::object`).
//!
//! Shapes are read once per object tag, in the object's model space, and
//! placed per placement by the placement's transform
//! ([`crate::scene::model_to_world`]). Each shape is posed by its node's
//! rest transform first: the `hlmt` nodes (`+0x78`, translation `+0xc`
//! and orientation `+0x18` relative to the parent), composed from the
//! roots ([`crate::scene::rest_frames`]). A one-node object at rest is
//! the identity, so its shapes sit at the placement's transform.
//!
//! Which node a shape follows:
//! - a `coll` BSP: the BSP element's leading word, a node index (V on
//!   every retail object, not proven against the engine;
//!   docs/formats/collision.md, "Not established"), named through the
//!   `coll` nodes and found in the `hlmt` nodes by that name;
//! - a `phmo` shape: its rigid body's node (`+0`), named through the
//!   `phmo` nodes the same way.
//!
//! A node index with no name to match is used as an `hlmt` node index
//! directly; one out of range leaves the shape unposed. Each region's
//! first permutation is drawn, as the viewport draws render models. A
//! sphere's and a box's transform shape applies its rotation rows to the
//! point (`R * v`, as the OBJ export does; whether the stored rotations act
//! on rows or columns is not established).
use std::collections::HashMap;
use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};
use tags::collision::{Collision, PhysicsModel, ShapeRef, ShapeType, Transform};
use tags::object::{Context, ResolvedModel};
use tags::wire::StringId;

use crate::geometry::{Marker, Mat4, Vec3, add, cross, dot, normalize, scale, sub};
use crate::scene::{RestNode, model_to_world, rest_frames};

/// What a shape is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    /// A `coll` BSP, drawn as its edges.
    Bsp,
    Sphere,
    Pill,
    Box,
    Polyhedron,
}

impl ShapeKind {
    /// The name the agent API uses.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bsp => "bsp",
            Self::Sphere => "sphere",
            Self::Pill => "pill",
            Self::Box => "box",
            Self::Polyhedron => "polyhedron",
        }
    }
}

/// One shape of an object's collision, in its model space, posed at rest.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelShape {
    pub kind: ShapeKind,
    /// The `coll` or `phmo` tag it comes from.
    pub tag: TagId,
    /// Its index: a BSP's in the order the regions' first permutations
    /// list them, a shape's in its own `phmo` block.
    pub index: usize,
    /// The `hlmt` node it is posed by.
    pub node: Option<usize>,
    /// Wire edges.
    pub lines: Vec<[Vec3; 2]>,
    pub center: Vec3,
    pub radius: f32,
}

/// An object tag's collision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelCollision {
    /// The object whose model it is (a collection's first entry).
    pub object: Option<TagId>,
    pub model: Option<TagId>,
    pub coll: Option<TagId>,
    pub phmo: Option<TagId>,
    /// The `hlmt` nodes.
    pub nodes: usize,
    pub shapes: Vec<ModelShape>,
    /// What did not read (a tag, a node chain).
    pub errors: Vec<String>,
}

/// One shape placed by one placement, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedShape<'a> {
    pub marker: &'a Marker,
    pub shape: &'a ModelShape,
    pub lines: Vec<[Vec3; 2]>,
    pub center: Vec3,
    pub radius: f32,
}

/// Every placed object tag's collision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectCollisions {
    models: HashMap<TagId, ModelCollision>,
    /// Why nothing read, when the map's tag metadata did not.
    pub error: Option<String>,
}

/// Segments in a circle.
const CIRCLE: usize = 16;
/// A polyhedron with more planes than this is drawn without edges.
const MAX_HULL_PLANES: usize = 256;
/// How deep a `phmo` shape list may nest.
const MAX_SHAPE_DEPTH: usize = 8;

impl ObjectCollisions {
    /// Read the collision of every object tag in `objects` (each once).
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        objects: impl IntoIterator<Item = TagId>,
    ) -> Self {
        let mut out = Self::default();
        out.add(map, objects);
        out
    }

    /// Read the collision of each tag in `objects` not read yet: a
    /// placement switched to a palette entry, or an item reference set to
    /// a tag, that nothing placed when the map opened. A tag that failed
    /// to read is kept with its errors and not read again. With no new
    /// tag the map's metadata is not read.
    pub fn add<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        objects: impl IntoIterator<Item = TagId>,
    ) {
        let mut new: Vec<TagId> = objects
            .into_iter()
            .filter(|o| !self.models.contains_key(o))
            .collect();
        new.sort_by_key(|o| o.0);
        new.dedup();
        if new.is_empty() {
            return;
        }
        let ctx = match Context::read(map) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        for object in new {
            let read = model_collision(map, &ctx, object);
            self.models.insert(object, read);
        }
    }

    /// The collision of an object tag, if it was read.
    pub fn of(&self, object: TagId) -> Option<&ModelCollision> {
        self.models.get(&object)
    }

    /// Every object tag read, in tag order.
    pub fn models(&self) -> Vec<(TagId, &ModelCollision)> {
        let mut out: Vec<_> = self.models.iter().map(|(&id, m)| (id, m)).collect();
        out.sort_by_key(|(id, _)| id.0);
        out
    }

    /// How many shapes `markers` place.
    pub fn count<'a>(&self, markers: impl IntoIterator<Item = &'a Marker>) -> usize {
        markers
            .into_iter()
            .filter_map(|m| self.of(m.object?))
            .map(|c| c.shapes.len())
            .sum()
    }

    /// Every shape `markers` place, in world space.
    pub fn placed<'a>(
        &'a self,
        markers: impl IntoIterator<Item = &'a Marker>,
    ) -> Vec<PlacedShape<'a>> {
        let mut out = Vec::new();
        for m in markers {
            let Some(c) = m.object.and_then(|o| self.of(o)) else {
                continue;
            };
            for shape in &c.shapes {
                out.push(PlacedShape {
                    marker: m,
                    shape,
                    lines: shape
                        .lines
                        .iter()
                        .map(|l| l.map(|p| model_to_world(m, p)))
                        .collect(),
                    center: model_to_world(m, shape.center),
                    radius: shape.radius * m.scale,
                });
            }
        }
        out
    }
}

/// An object's model: an item or vehicle collection stands for its first
/// entry, as the viewport draws it.
fn resolve(ctx: &Context, object: TagId) -> Result<(TagId, Option<ResolvedModel>), String> {
    let group = ctx.catalog.get(object).ok_or("no such tag")?.group;
    let object = if group == FourCC(*b"itmc") || group == FourCC(*b"vehc") {
        let tag = ctx.decode(object).map_err(|e| e.to_string())?;
        tag.layers
            .first()
            .map(|l| l.elements("entries"))
            .unwrap_or(&[])
            .iter()
            .filter_map(|e| e.tag_ref("object"))
            .map(|r| r.id)
            .find(|id| *id != TagId::NULL)
            .ok_or("the collection has no entry")?
    } else {
        object
    };
    let resolved = ctx.resolve_object(object).map_err(|e| e.to_string())?;
    Ok((object, resolved.model))
}

/// The `hlmt` nodes' names and rest transforms in model space.
fn rest_nodes(ctx: &Context, model: &ResolvedModel) -> Result<(Vec<StringId>, Vec<Mat4>), String> {
    let records = model
        .tag
        .layers
        .first()
        .map(|l| l.elements("nodes"))
        .unwrap_or(&[]);
    let mut names = Vec::with_capacity(records.len());
    let mut nodes = Vec::with_capacity(records.len());
    for r in records {
        let b = ctx
            .meta
            .slice(r.address, 0x5c)
            .map_err(|e| format!("hlmt node: {e}"))?;
        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        names.push(r.sid("name").map_or(StringId(0), |s| s.0));
        nodes.push(RestNode {
            parent: i16::from_le_bytes([b[4], b[5]]),
            translation: [f(0xc), f(0x10), f(0x14)],
            orientation: [f(0x18), f(0x1c), f(0x20), f(0x24)],
        });
    }
    Ok((names, rest_frames(&nodes)))
}

/// The `hlmt` node a tag's node index names: through the tag's own node
/// names when it has them, else the index itself.
fn node_of(index: i32, own: &[StringId], model: &[StringId]) -> Option<usize> {
    let i = usize::try_from(index).ok()?;
    match own.get(i) {
        Some(name) => model.iter().position(|n| n == name),
        None if own.is_empty() => (i < model.len()).then_some(i),
        None => None,
    }
}

fn model_collision<R: Read + Seek>(
    map: &mut Map<R>,
    ctx: &Context,
    object: TagId,
) -> ModelCollision {
    let mut out = ModelCollision::default();
    let model = match resolve(ctx, object) {
        Ok((o, model)) => {
            out.object = Some(o);
            match model {
                Some(m) => m,
                None => return out,
            }
        }
        Err(e) => {
            out.errors.push(e);
            return out;
        }
    };
    out.model = Some(model.id);
    out.coll = model.collision_model.as_ref().map(|l| l.id);
    out.phmo = model.physics_model.as_ref().map(|l| l.id);
    let (names, rest) = rest_nodes(ctx, &model).unwrap_or_else(|e| {
        out.errors.push(e);
        (Vec::new(), Vec::new())
    });
    out.nodes = names.len();
    let pose = |node: Option<usize>, p: Vec3| match node.and_then(|n| rest.get(n)) {
        Some(m) => {
            let v = crate::geometry::transform(m, [p[0], p[1], p[2], 1.0]);
            [v[0], v[1], v[2]]
        }
        None => p,
    };
    if let Some(id) = out.coll {
        match Collision::read(map, id) {
            Ok(coll) => {
                let own: Vec<StringId> = coll.nodes.iter().map(|n| n.name).collect();
                let mut index = 0;
                for region in &coll.regions {
                    let Some(perm) = region.permutations.first() else {
                        continue;
                    };
                    for bsp in &perm.bsps {
                        let node = node_of((bsp.unknown_00 & 0xffff) as i16 as i32, &own, &names);
                        let point = |v: u16| {
                            bsp.vertices
                                .get(usize::from(v))
                                .map(|v| pose(node, v.point))
                        };
                        let lines: Vec<[Vec3; 2]> = bsp
                            .edges
                            .iter()
                            .filter_map(|e| Some([point(e.start_vertex)?, point(e.end_vertex)?]))
                            .collect();
                        let points: Vec<Vec3> =
                            bsp.vertices.iter().map(|v| pose(node, v.point)).collect();
                        let (center, radius) = sphere_round(&points);
                        out.shapes.push(ModelShape {
                            kind: ShapeKind::Bsp,
                            tag: id,
                            index,
                            node,
                            lines,
                            center,
                            radius,
                        });
                        index += 1;
                    }
                }
            }
            Err(e) => out.errors.push(format!("coll: {e}")),
        }
    }
    if let Some(id) = out.phmo {
        match PhysicsModel::read(map, id) {
            Ok(phmo) => {
                let own: Vec<StringId> = phmo.nodes.iter().map(|n| n.name).collect();
                for body in &phmo.rigid_bodies {
                    let node = node_of(i32::from(body.node), &own, &names);
                    let mut shapes = Vec::new();
                    leaf_shapes(&phmo, body.shape, 0, &mut shapes);
                    for (kind, index, lines) in shapes {
                        let lines: Vec<[Vec3; 2]> = lines
                            .into_iter()
                            .map(|l| l.map(|p| pose(node, p)))
                            .collect();
                        let points: Vec<Vec3> = lines.iter().flatten().copied().collect();
                        let (center, radius) = match kind {
                            ShapeKind::Sphere => {
                                let s = &phmo.spheres[index];
                                (pose(node, havok(&s.transform, [0.0; 3])), s.radius)
                            }
                            _ => sphere_round(&points),
                        };
                        out.shapes.push(ModelShape {
                            kind,
                            tag: id,
                            index,
                            node,
                            lines,
                            center,
                            radius,
                        });
                    }
                }
            }
            Err(e) => out.errors.push(format!("phmo: {e}")),
        }
    }
    out
}

type Leaf = (ShapeKind, usize, Vec<[Vec3; 2]>);

/// The drawable shapes under `r`, in the rigid body's space.
fn leaf_shapes(phmo: &PhysicsModel, r: ShapeRef, depth: usize, out: &mut Vec<Leaf>) {
    if depth > MAX_SHAPE_DEPTH {
        return;
    }
    let i = usize::from(r.index);
    match r.kind {
        ShapeType::Sphere => {
            if let Some(s) = phmo.spheres.get(i) {
                let c = havok(&s.transform, [0.0; 3]);
                out.push((ShapeKind::Sphere, i, sphere_lines(c, s.radius)));
            }
        }
        ShapeType::Pill => {
            if let Some(p) = phmo.pills.get(i) {
                out.push((ShapeKind::Pill, i, pill_lines(p.bottom, p.top, p.radius)));
            }
        }
        ShapeType::Box => {
            if let Some(b) = phmo.boxes.get(i) {
                out.push((ShapeKind::Box, i, box_lines(&b.transform, b.half_extents)));
            }
        }
        ShapeType::Polyhedron => {
            if let Some(p) = phmo.polyhedra.get(i) {
                out.push((ShapeKind::Polyhedron, i, hull_lines(&p.vertices, &p.planes)));
            }
        }
        ShapeType::List => {
            for c in phmo.lists.get(i).map_or(&[][..], |l| l.children.as_slice()) {
                leaf_shapes(phmo, c.shape, depth + 1, out);
            }
        }
        ShapeType::Mopp => {
            if let Some(m) = phmo.mopps.get(i) {
                leaf_shapes(phmo, m.child, depth + 1, out);
            }
        }
        ShapeType::Type6 => {
            if let Some(t) = phmo.type6.get(i) {
                leaf_shapes(phmo, t.child, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// A transform shape applied to a point: each rotation row dotted with it,
/// then the translation.
pub fn havok(t: &Transform, p: Vec3) -> Vec3 {
    add(t.rotation.map(|row| dot(row, p)), t.translation)
}

fn circle(c: Vec3, u: Vec3, v: Vec3, r: f32) -> Vec<[Vec3; 2]> {
    let at = |k: usize| {
        let a = k as f32 / CIRCLE as f32 * std::f32::consts::TAU;
        add(c, add(scale(u, r * a.cos()), scale(v, r * a.sin())))
    };
    (0..CIRCLE).map(|k| [at(k), at(k + 1)]).collect()
}

/// Three great circles.
pub fn sphere_lines(c: Vec3, r: f32) -> Vec<[Vec3; 2]> {
    let (x, y, z) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let mut out = circle(c, x, y, r);
    out.extend(circle(c, y, z, r));
    out.extend(circle(c, z, x, r));
    out
}

/// A circle round each end and four lines along the side.
pub fn pill_lines(a: Vec3, b: Vec3, r: f32) -> Vec<[Vec3; 2]> {
    let axis = normalize(sub(b, a));
    let axis = if dot(axis, axis) < 0.5 {
        [0.0, 0.0, 1.0]
    } else {
        axis
    };
    let helper = if axis[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalize(cross(helper, axis));
    let v = cross(axis, u);
    let mut out = circle(a, u, v, r);
    out.extend(circle(b, u, v, r));
    for d in [u, v, scale(u, -1.0), scale(v, -1.0)] {
        out.push([add(a, scale(d, r)), add(b, scale(d, r))]);
    }
    out
}

/// A box's twelve edges.
pub fn box_lines(t: &Transform, h: Vec3) -> Vec<[Vec3; 2]> {
    let corner = |k: usize| {
        let s = |bit: usize| if k & bit != 0 { 1.0 } else { -1.0 };
        havok(t, [s(1) * h[0], s(2) * h[1], s(4) * h[2]])
    };
    let mut out = Vec::with_capacity(12);
    for k in 0..8 {
        for bit in [1, 2, 4] {
            if k & bit == 0 {
                out.push([corner(k), corner(k | bit)]);
            }
        }
    }
    out
}

/// A convex hull's edges from its vertices and planes (`n . p + w = 0` on
/// a face): for each pair of planes, the two farthest vertices on both.
pub fn hull_lines(vertices: &[Vec3], planes: &[[f32; 4]]) -> Vec<[Vec3; 2]> {
    if planes.len() > MAX_HULL_PLANES {
        return Vec::new();
    }
    let extent = vertices
        .iter()
        .map(|p| dot(*p, *p).sqrt())
        .fold(1.0f32, f32::max);
    let eps = 1e-3 * extent;
    let on: Vec<Vec<usize>> = planes
        .iter()
        .map(|q| {
            (0..vertices.len())
                .filter(|&i| (dot([q[0], q[1], q[2]], vertices[i]) + q[3]).abs() <= eps)
                .collect()
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..planes.len() {
        for j in i + 1..planes.len() {
            let both: Vec<usize> = on[i]
                .iter()
                .copied()
                .filter(|v| on[j].contains(v))
                .collect();
            let mut best: Option<(f32, usize, usize)> = None;
            for (k, &a) in both.iter().enumerate() {
                for &b in &both[k + 1..] {
                    let d = sub(vertices[a], vertices[b]);
                    let d = dot(d, d);
                    if best.is_none_or(|x| d > x.0) {
                        best = Some((d, a, b));
                    }
                }
            }
            if let Some((d, a, b)) = best
                && d > eps * eps
            {
                out.push([vertices[a], vertices[b]]);
            }
        }
    }
    out
}

/// A sphere round points: their bounds' centre and the farthest point.
fn sphere_round(points: &[Vec3]) -> (Vec3, f32) {
    if points.is_empty() {
        return ([0.0; 3], 0.0);
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = scale(add(lo, hi), 0.5);
    let r = points
        .iter()
        .map(|&p| {
            let d = sub(p, c);
            dot(d, d).sqrt()
        })
        .fold(0.0, f32::max);
    (c, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Transform {
        Transform {
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0; 3],
        }
    }

    /// A unit cube's hull has its twelve edges; a box has twelve edges
    /// round its centre; a sphere is three circles at its radius.
    #[test]
    fn shapes_draw_as_their_edges() {
        let mut vertices = Vec::new();
        for k in 0..8 {
            let s = |bit: usize| if k & bit != 0 { 1.0 } else { -1.0 };
            vertices.push([s(1), s(2), s(4)]);
        }
        let mut planes = Vec::new();
        for axis in 0..3 {
            for sign in [1.0, -1.0] {
                let mut n = [0.0; 3];
                n[axis] = sign;
                planes.push([n[0], n[1], n[2], -1.0]);
            }
        }
        let hull = hull_lines(&vertices, &planes);
        assert_eq!(hull.len(), 12);
        for [a, b] in &hull {
            let d = sub(*a, *b);
            assert!((dot(d, d) - 4.0).abs() < 1e-5, "{a:?} {b:?}");
        }

        let mut t = identity();
        t.translation = [0.0, 0.0, 3.0];
        let edges = box_lines(&t, [0.5, 1.0, 2.0]);
        assert_eq!(edges.len(), 12);
        assert!(edges.iter().flatten().all(|p| (p[2] - 3.0).abs() == 2.0));
        // A turned box: its rows map x to y.
        t.rotation = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        assert_eq!(havok(&t, [1.0, 0.0, 0.0]), [0.0, 1.0, 3.0]);

        let circles = sphere_lines([1.0, 2.0, 3.0], 0.5);
        assert_eq!(circles.len(), 3 * CIRCLE);
        for [a, _] in circles {
            let d = sub(a, [1.0, 2.0, 3.0]);
            assert!((dot(d, d).sqrt() - 0.5).abs() < 1e-5);
        }
        assert_eq!(
            pill_lines([0.0; 3], [0.0, 0.0, 2.0], 0.25).len(),
            2 * CIRCLE + 4
        );
    }

    /// A shape's node name finds the `hlmt` node; a tag without names
    /// uses the index; an unknown name or index leaves it unposed.
    #[test]
    fn nodes_match_by_name_then_index() {
        let model = [StringId(10), StringId(11), StringId(12)];
        assert_eq!(node_of(0, &[StringId(12)], &model), Some(2));
        assert_eq!(node_of(0, &[StringId(99)], &model), None);
        assert_eq!(node_of(1, &[], &model), Some(1));
        assert_eq!(node_of(5, &[], &model), None);
        assert_eq!(node_of(-1, &[], &model), None);
    }
}
