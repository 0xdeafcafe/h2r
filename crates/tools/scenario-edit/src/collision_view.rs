//! The structure BSPs' collision as the viewport shows it
//! (docs/editor/polish-plan.md, slice P10): every surface of each scenario
//! BSP's collision BSP as a polygon, and which of them no drawn surface
//! covers, the invisible walls.
//!
//! A structure BSP (`sbsp`) holds one collision BSP in the block at
//! `+0x14` (`tags::pathfinding::SBSP_COLLISION_FIELD`), read by
//! [`tags::collision::Bsp::decode_structure`]. A surface's polygon is its
//! edge ring ([`Bsp::surface_ring`]); its plane is the plane its index
//! names, flipped when bit 15 is set.
//!
//! [`classify`] is geometric and needs no surface flag to be named: a
//! surface is [`SurfaceClass::CollisionOnly`] when no render triangle of
//! the structure or its instances lies on its plane (every corner within
//! [`PLANE_TOLERANCE`], the normal within [`NORMAL_COS`] either way) and
//! overlaps it by more than [`MIN_OVERLAP`]. The raw flags, breakable
//! surface and material are carried for the legend and the agent API.
//!
//! Instanced geometry has its own collision: each definition's collision
//! BSP ([`tags::structure::InstancedGeometryDefinition::collision`]),
//! placed by each placement that names it ([`instance_surfaces`]; slice
//! P11). Those surfaces are classified with the structure's.
use std::collections::HashMap;
use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::collision::Bsp;
use tags::geometry::TagView;
use tags::pathfinding::SBSP_COLLISION_FIELD;

use crate::geometry::{self, Ray, Tri, Vec3, WorldPart, add, cross, dot, normalize, scale, sub};

/// How far (world units) a render triangle's corners may lie from a
/// collision surface's plane and still cover it.
pub const PLANE_TOLERANCE: f32 = 0.01;
/// The least `|cos|` between a render triangle's normal and the surface's
/// for it to lie on the plane (about five degrees). Either facing counts:
/// a face drawn from behind is still drawn.
pub const NORMAL_COS: f32 = 0.996;
/// The least overlap (square world units) that counts as covering, so a
/// triangle that only touches an edge does not.
pub const MIN_OVERLAP: f32 = 1e-4;

/// What a collision surface is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SurfaceClass {
    /// A render triangle covers it.
    Drawn,
    /// Nothing drawn covers it: an invisible wall, floor or ceiling.
    CollisionOnly,
}

impl SurfaceClass {
    pub const ALL: [Self; 2] = [Self::Drawn, Self::CollisionOnly];

    /// The name the agent API uses.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Drawn => "drawn",
            Self::CollisionOnly => "collision_only",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// One collision surface of one structure BSP.
#[derive(Clone, Debug, PartialEq)]
pub struct CollisionSurface {
    /// The scenario BSP's index (`scnr+0x210`).
    pub bsp: usize,
    /// The surface's index in that BSP's collision BSP, or in its
    /// instance definition's.
    pub surface: usize,
    /// The instanced geometry placement (`sbsp+0x140` index) that places
    /// this surface; `None` for the structure's own collision.
    pub instance: Option<usize>,
    /// `[nx, ny, nz, d]` with `n . p = d`, the flip bit applied.
    pub plane: [f32; 4],
    /// The raw flags byte (bit 3 is breakable; the rest are not named).
    pub flags: u8,
    /// The breakable surface index, 0xff for none.
    pub breakable: u8,
    pub material: u16,
    /// The ring's points, in ring order.
    pub ring: Vec<Vec3>,
    pub class: SurfaceClass,
    /// The share of the surface's area that render triangles on its plane
    /// cover, clamped to 1 (triangles that overlap each other count twice).
    pub covered: f32,
}

impl CollisionSurface {
    pub fn normal(&self) -> Vec3 {
        [self.plane[0], self.plane[1], self.plane[2]]
    }

    /// The ring as a fan from its first point: triangle `k` is ring points
    /// 0, `k + 1` and `k + 2`.
    pub fn triangles(&self) -> Vec<[Vec3; 3]> {
        fan(&self.ring)
    }

    pub fn area(&self) -> f32 {
        self.triangles().iter().map(tri_area).sum()
    }

    pub fn center(&self) -> Vec3 {
        let n = self.ring.len().max(1) as f32;
        scale(self.ring.iter().fold([0.0; 3], |a, &p| add(a, p)), 1.0 / n)
    }

    /// The farthest ring point from [`Self::center`].
    pub fn radius(&self) -> f32 {
        let c = self.center();
        self.ring
            .iter()
            .map(|&p| {
                let d = sub(p, c);
                dot(d, d).sqrt()
            })
            .fold(0.0, f32::max)
    }

    /// The ring's edges.
    pub fn edges(&self) -> Vec<[Vec3; 2]> {
        let n = self.ring.len();
        (0..n)
            .filter(|_| n >= 2)
            .map(|i| [self.ring[i], self.ring[(i + 1) % n]])
            .collect()
    }

    /// Distance along the ray to the polygon, from either side.
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        self.triangles()
            .iter()
            .filter_map(|t| ray.triangle(t))
            .min_by(f32::total_cmp)
    }

    /// How far `p` is from the polygon's plane, when it lies over the
    /// polygon.
    pub fn distance(&self, p: Vec3) -> Option<f32> {
        let n = self.normal();
        let h = dot(n, p) - self.plane[3];
        let foot = sub(p, scale(n, h));
        let inside = self.triangles().iter().any(|t| {
            let a = cross(sub(t[1], t[0]), sub(foot, t[0]));
            let b = cross(sub(t[2], t[1]), sub(foot, t[1]));
            let c = cross(sub(t[0], t[2]), sub(foot, t[2]));
            let (a, b, c) = (dot(a, n), dot(b, n), dot(c, n));
            let eps = -1e-5;
            (a >= eps && b >= eps && c >= eps) || (a <= -eps && b <= -eps && c <= -eps)
        });
        inside.then_some(h.abs())
    }
}

fn fan(ring: &[Vec3]) -> Vec<[Vec3; 3]> {
    (1..ring.len().saturating_sub(1))
        .map(|k| [ring[0], ring[k], ring[k + 1]])
        .collect()
}

fn tri_area(t: &[Vec3; 3]) -> f32 {
    let c = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    0.5 * dot(c, c).sqrt()
}

/// Every surface of a collision BSP, unclassified ([`SurfaceClass::Drawn`],
/// nothing covered). A surface whose plane index is out of range is left
/// out.
pub fn surfaces_of(bsp_index: usize, bsp: &Bsp) -> Vec<CollisionSurface> {
    bsp.surfaces
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let tags::collision::Plane(p) = *bsp.planes.get(usize::from(s.plane & 0x7fff))?;
            let plane = if s.plane & 0x8000 != 0 {
                p.map(|v| -v)
            } else {
                p
            };
            let ring = bsp
                .surface_ring(i)
                .into_iter()
                .filter_map(|v| bsp.vertices.get(v).map(|v| v.point))
                .collect();
            Some(CollisionSurface {
                bsp: bsp_index,
                surface: i,
                instance: None,
                plane,
                flags: s.flags,
                breakable: s.breakable_surface,
                material: s.material,
                ring,
                class: SurfaceClass::Drawn,
                covered: 0.0,
            })
        })
        .collect()
}

/// Every triangle the viewport draws for the structure and its instances,
/// transparent parts included (glass is seen).
pub fn render_triangles(parts: &[WorldPart]) -> Vec<Tri> {
    let mut out = Vec::new();
    for part in parts {
        for t in &part.tris {
            let p = t.map(|i| part.positions[i as usize]);
            let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
            out.push(Tri { p, n });
        }
    }
    out
}

/// A grid of render triangles by their bounds, for [`classify`].
struct Grid {
    cell: f32,
    cells: HashMap<[i32; 3], Vec<u32>>,
    /// Triangles too large to bucket, checked against every surface.
    large: Vec<u32>,
}

/// Cells one triangle may span before it goes in [`Grid::large`].
const MAX_CELLS: i64 = 512;

impl Grid {
    fn new(tris: &[Tri], cell: f32) -> Self {
        let mut grid = Self {
            cell,
            cells: HashMap::new(),
            large: Vec::new(),
        };
        for (i, t) in tris.iter().enumerate() {
            let (lo, hi) = grid.span(&t.p, 0.0);
            let n: i64 = (0..3).map(|k| i64::from(hi[k] - lo[k] + 1)).product();
            if n > MAX_CELLS {
                grid.large.push(i as u32);
                continue;
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        grid.cells.entry([x, y, z]).or_default().push(i as u32);
                    }
                }
            }
        }
        grid
    }

    fn span(&self, points: &[Vec3], pad: f32) -> ([i32; 3], [i32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k] - pad);
                hi[k] = hi[k].max(p[k] + pad);
            }
        }
        let cell = |v: f32| (v / self.cell).floor().clamp(-1e6, 1e6) as i32;
        (lo.map(cell), hi.map(cell))
    }

    /// Every triangle in a cell the points' bounds (grown by `pad`) touch,
    /// once each.
    fn near(&self, points: &[Vec3], pad: f32, stamp: &mut [u32], mark: u32) -> Vec<u32> {
        let (lo, hi) = self.span(points, pad);
        let mut out: Vec<u32> = Vec::new();
        let mut take = |i: u32| {
            if stamp[i as usize] != mark {
                stamp[i as usize] = mark;
                out.push(i);
            }
        };
        let cells: i64 = (0..3).map(|k| i64::from(hi[k] - lo[k] + 1)).product();
        if cells > MAX_CELLS * 64 {
            // A huge surface: walk the occupied cells instead.
            for (c, tris) in &self.cells {
                if (0..3).all(|k| (lo[k]..=hi[k]).contains(&c[k])) {
                    tris.iter().for_each(|&i| take(i));
                }
            }
        } else {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        if let Some(tris) = self.cells.get(&[x, y, z]) {
                            tris.iter().for_each(|&i| take(i));
                        }
                    }
                }
            }
        }
        self.large.iter().for_each(|&i| take(i));
        out
    }
}

/// A 2D basis on a plane with normal `n`.
fn basis(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalize(cross(helper, n));
    (u, cross(n, u))
}

type P2 = [f32; 2];

fn cross2(o: P2, a: P2, b: P2) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn polygon_area(p: &[P2]) -> f32 {
    let n = p.len();
    0.5 * (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f32>()
        .abs()
}

/// The part of `subject` inside the convex `clip` (Sutherland-Hodgman).
fn clip(subject: &[P2], clip: &[P2; 3]) -> Vec<P2> {
    // Counter-clockwise, so "inside" is to the left of each edge.
    let clip = if cross2(clip[0], clip[1], clip[2]) < 0.0 {
        [clip[0], clip[2], clip[1]]
    } else {
        *clip
    };
    let mut out = subject.to_vec();
    for i in 0..3 {
        let (a, b) = (clip[i], clip[(i + 1) % 3]);
        let input = std::mem::take(&mut out);
        let n = input.len();
        for j in 0..n {
            let (p, q) = (input[j], input[(j + 1) % n]);
            let (dp, dq) = (cross2(a, b, p), cross2(a, b, q));
            if dp >= 0.0 {
                out.push(p);
            }
            if (dp >= 0.0) != (dq >= 0.0) {
                let t = dp / (dp - dq);
                out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Classify every surface against the render triangles: see the module
/// docs. Sets [`CollisionSurface::class`] and [`CollisionSurface::covered`].
pub fn classify(surfaces: &mut [CollisionSurface], render: &[Tri]) {
    let grid = Grid::new(render, 8.0);
    let mut stamp = vec![0u32; render.len()];
    for (k, s) in surfaces.iter_mut().enumerate() {
        let area = s.area();
        let fan3 = s.triangles();
        if area <= MIN_OVERLAP || fan3.is_empty() {
            // Nothing to see either way.
            s.class = SurfaceClass::Drawn;
            s.covered = 0.0;
            continue;
        }
        let n = s.normal();
        let (u, v) = basis(n);
        let flat = |p: Vec3| [dot(p, u), dot(p, v)];
        let fan2: Vec<[P2; 3]> = fan3.iter().map(|t| t.map(flat)).collect();
        let mut overlap = 0.0f32;
        let mut covering = false;
        let mark = k as u32 + 1;
        for i in grid.near(&s.ring, PLANE_TOLERANCE, &mut stamp, mark) {
            let t = &render[i as usize];
            if dot(t.n, n).abs() < NORMAL_COS
                || t.p
                    .iter()
                    .any(|p| (dot(n, *p) - s.plane[3]).abs() > PLANE_TOLERANCE)
            {
                continue;
            }
            let tri2 = t.p.map(flat);
            let a: f32 = fan2
                .iter()
                .map(|f| {
                    let part = clip(&tri2, f);
                    if part.len() >= 3 {
                        polygon_area(&part)
                    } else {
                        0.0
                    }
                })
                .sum();
            if a > MIN_OVERLAP {
                covering = true;
                overlap += a;
            }
        }
        s.class = if covering {
            SurfaceClass::Drawn
        } else {
            SurfaceClass::CollisionOnly
        };
        s.covered = (overlap / area).min(1.0);
    }
}

/// The collision BSP of a structure BSP payload loaded at `base`, or
/// `None` when its collision block is empty.
pub fn structure_collision(bytes: &[u8], base: u32) -> crate::Result<Option<Bsp>> {
    let view = TagView::new(bytes, base);
    view.at(0, SBSP_COLLISION_FIELD + 8)?;
    let records = view.block(view.bytes(), SBSP_COLLISION_FIELD, Bsp::STRUCTURE_SIZE)?;
    let Some(record) = records.first() else {
        return Ok(None);
    };
    let at = record.as_ptr() as usize - bytes.as_ptr() as usize;
    Ok(Some(Bsp::decode_structure(bytes, base, at)?))
}

/// Every instanced geometry placement's collision surfaces of a structure
/// BSP payload loaded at `base`, in world space: each definition's
/// collision BSP moved by the placement's scale, basis and position
/// ([`tags::structure::InstancedGeometryInstance::transform`]).
pub fn instance_surfaces(
    bsp_index: usize,
    bytes: &[u8],
    base: u32,
) -> crate::Result<Vec<CollisionSurface>> {
    use tags::structure::{InstancedGeometryDefinition, InstancedGeometryInstance};
    let view = TagView::new(bytes, base);
    let definitions = tags::structure::instance_definitions(&view)?;
    let placements = InstancedGeometryInstance::placements(&view, definitions.len())?;
    let mut local: Vec<Option<Vec<CollisionSurface>>> = vec![None; definitions.len()];
    let mut out = Vec::new();
    for (index, p) in placements.iter().enumerate() {
        let d = p.definition as usize;
        if local[d].is_none() {
            let bsp = InstancedGeometryDefinition::collision(&view, definitions[d])?;
            local[d] = Some(surfaces_of(bsp_index, &bsp));
        }
        for s in local[d].iter().flatten() {
            let n = normalize(p.rotate(s.normal()));
            let on_plane = p.transform(scale(s.normal(), s.plane[3]));
            out.push(CollisionSurface {
                instance: Some(index),
                plane: [n[0], n[1], n[2], dot(n, on_plane)],
                ring: s.ring.iter().map(|&q| p.transform(q)).collect(),
                ..s.clone()
            });
        }
    }
    Ok(out)
}

/// Every scenario BSP's collision surfaces, classified.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelCollision {
    /// The scenario's BSP tags, in order.
    pub bsps: Vec<TagId>,
    pub surfaces: Vec<CollisionSurface>,
    /// BSPs whose collision did not read: (scenario BSP index, why).
    pub errors: Vec<(usize, String)>,
}

impl LevelCollision {
    /// Read every scenario BSP's collision BSP and classify its surfaces
    /// against `render` ([`render_triangles`]). A BSP that does not read is
    /// named in [`Self::errors`]; the others still read.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, render: &[Tri]) -> crate::Result<Self> {
        let mut out = Self {
            bsps: geometry::scenario_bsps(map)?,
            ..Self::default()
        };
        for (i, &id) in out.bsps.clone().iter().enumerate() {
            type Got = (Option<Bsp>, crate::Result<Vec<CollisionSurface>>);
            let read = (|| -> crate::Result<Got> {
                let address = map
                    .tag(id)?
                    .location
                    .ok_or(crate::Error::Invalid("BSP has no payload"))?
                    .address;
                let bytes = map.read_tag_bytes(id)?;
                Ok((
                    structure_collision(&bytes, address)?,
                    instance_surfaces(i, &bytes, address),
                ))
            })();
            match read {
                Ok((bsp, instances)) => {
                    if let Some(bsp) = bsp {
                        out.surfaces.extend(surfaces_of(i, &bsp));
                    }
                    match instances {
                        Ok(s) => out.surfaces.extend(s),
                        Err(e) => out.errors.push((i, format!("instances: {e}"))),
                    }
                }
                Err(e) => out.errors.push((i, e.to_string())),
            }
        }
        classify(&mut out.surfaces, render);
        Ok(out)
    }

    pub fn count(&self, class: SurfaceClass) -> usize {
        self.surfaces.iter().filter(|s| s.class == class).count()
    }

    /// How many surfaces instanced geometry places.
    pub fn instance_count(&self) -> usize {
        self.surfaces
            .iter()
            .filter(|s| s.instance.is_some())
            .count()
    }

    /// The nearest surface the ray meets among those `shown` accepts.
    pub fn pick(
        &self,
        ray: &Ray,
        shown: impl Fn(&CollisionSurface) -> bool,
    ) -> Option<(f32, usize)> {
        self.surfaces
            .iter()
            .enumerate()
            .filter(|(_, s)| shown(*s))
            .filter_map(|(i, s)| s.hit(ray).map(|t| (t, i)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }

    /// The surface nearest `p` within `within` of its plane, over its
    /// polygon.
    pub fn at_point(&self, p: Vec3, within: f32) -> Option<(f32, usize)> {
        self.surfaces
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.distance(p).map(|d| (d, i)))
            .filter(|(d, _)| *d <= within)
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    /// The invented room at the origin ([`fixture::collision_room`]): a
    /// floor, a wall at x 0 and a wall at x 10.
    fn room() -> Bsp {
        fixture::collision_room([0.0; 3])
    }

    fn quad_tris(c: [Vec3; 4]) -> Vec<Tri> {
        [[c[0], c[1], c[2]], [c[0], c[2], c[3]]]
            .into_iter()
            .map(|p| Tri {
                p,
                n: normalize(cross(sub(p[1], p[0]), sub(p[2], p[0]))),
            })
            .collect()
    }

    /// The room's drawn faces: the floor, and the wall at x 0 drawn 0.001
    /// off its plane. The wall at x 10 has none.
    fn room_render() -> Vec<Tri> {
        let d = 0.001;
        let mut out = quad_tris([
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.0, 10.0, 0.0],
            [0.0, 10.0, 0.0],
        ]);
        out.extend(quad_tris([
            [d, 0.0, 0.0],
            [d, 10.0, 0.0],
            [d, 10.0, 4.0],
            [d, 0.0, 4.0],
        ]));
        out
    }

    #[test]
    fn the_wall_with_no_render_face_is_alone_in_collision_only() {
        let bsp = room();
        let mut surfaces = surfaces_of(0, &bsp);
        assert_eq!(surfaces.len(), 3);
        classify(&mut surfaces, &room_render());
        let classes: Vec<SurfaceClass> = surfaces.iter().map(|s| s.class).collect();
        assert_eq!(
            classes,
            [
                SurfaceClass::Drawn,
                SurfaceClass::Drawn,
                SurfaceClass::CollisionOnly
            ]
        );
        // The floor and the near wall are wholly covered, the far wall not.
        assert!((surfaces[0].covered - 1.0).abs() < 1e-4);
        assert!((surfaces[1].covered - 1.0).abs() < 1e-4, "0.001 off covers");
        assert_eq!(surfaces[2].covered, 0.0);
        assert_eq!(surfaces[2].flags, 0x08);

        // Moved a tenth of a unit out, the near wall's face no longer lies
        // on its plane.
        let mut far: Vec<Tri> = room_render();
        for t in &mut far[2..] {
            for p in &mut t.p {
                p[0] = 0.1;
            }
        }
        classify(&mut surfaces, &far);
        assert_eq!(surfaces[1].class, SurfaceClass::CollisionOnly);
        // A face that only touches the far wall's edge does not cover it.
        let mut touching = room_render();
        touching.extend(quad_tris([
            [10.0, 10.0, 0.0],
            [10.0, 12.0, 0.0],
            [10.0, 12.0, 4.0],
            [10.0, 10.0, 4.0],
        ]));
        classify(&mut surfaces, &touching);
        assert_eq!(surfaces[2].class, SurfaceClass::CollisionOnly);
    }

    #[test]
    fn triangulated_rings_follow_the_bsps_edge_walk() {
        let bsp = room();
        for s in surfaces_of(0, &bsp) {
            let ring: Vec<Vec3> = bsp
                .surface_ring(s.surface)
                .into_iter()
                .map(|v| bsp.vertices[v].point)
                .collect();
            assert_eq!(s.ring, ring);
            let tris = s.triangles();
            assert_eq!(tris.len(), ring.len() - 2);
            for (k, t) in tris.iter().enumerate() {
                assert_eq!(*t, [ring[0], ring[k + 1], ring[k + 2]]);
            }
            assert!((s.area() - if s.surface == 0 { 100.0 } else { 40.0 }).abs() < 1e-4);
        }
    }

    #[test]
    fn a_ray_or_a_point_finds_the_wall() {
        let mut level = LevelCollision {
            bsps: vec![TagId(1)],
            surfaces: surfaces_of(0, &room()),
            errors: Vec::new(),
        };
        classify(&mut level.surfaces, &room_render());
        let ray = Ray {
            origin: [5.0, 5.0, 1.0],
            dir: [1.0, 0.0, 0.0],
        };
        let (t, i) = level.pick(&ray, |_| true).unwrap();
        assert_eq!(i, 2);
        assert!((t - 5.0).abs() < 1e-4, "{t}");
        assert_eq!(level.pick(&ray, |s| s.class == SurfaceClass::Drawn), None);
        assert_eq!(level.at_point([9.99, 5.0, 1.0], 0.05).map(|h| h.1), Some(2));
        assert_eq!(level.at_point([5.0, 5.0, 2.0], 0.05), None);
        // Outside the polygon, however close to its plane.
        assert_eq!(level.at_point([10.0, 20.0, 2.0], 0.05), None);
        assert_eq!(level.count(SurfaceClass::CollisionOnly), 1);
        assert_eq!(level.count(SurfaceClass::Drawn), 2);
    }

    /// The structure's collision block reads back through the payload
    /// reader, as the session reads each scenario BSP.
    #[test]
    fn the_structure_collision_block_reads_back() {
        let base = 0x8000_0000u32;
        let mut bytes = vec![0u8; 0x200];
        let blocks_at = bytes.len();
        bytes.extend(room().encode_blocks(base + blocks_at as u32));
        bytes[SBSP_COLLISION_FIELD..SBSP_COLLISION_FIELD + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[SBSP_COLLISION_FIELD + 4..SBSP_COLLISION_FIELD + 8]
            .copy_from_slice(&(base + blocks_at as u32).to_le_bytes());
        let read = structure_collision(&bytes, base).unwrap().unwrap();
        assert_eq!(read.surfaces, room().surfaces);
        assert_eq!(read.vertices, room().vertices);
        let empty = vec![0u8; 0x200];
        assert_eq!(structure_collision(&empty, base).unwrap(), None);
    }
}
