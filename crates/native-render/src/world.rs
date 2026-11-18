//! The static world as GPU-ready arrays: one vertex buffer, one index
//! buffer, one draw per drawn part in world space, sorted into batches
//! that share a material and a lightmap page. Built once at map load;
//! pure data, no GPU and no disc (`crate::load` fills it from a map).

use bytemuck::{Pod, Zeroable};

/// One world vertex (40 bytes): what the lightmap pass reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WorldVertex {
    pub position: [f32; 3],
    /// Base-map coordinates (stream texcoord).
    pub uv: [f32; 2],
    /// Lightmap page coordinates (stream `1f`), zero without a lightmap.
    pub lightmap_uv: [f32; 2],
    pub normal: [f32; 3],
}

/// Axis-aligned bounds in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn add(&mut self, p: [f32; 3]) {
        for k in 0..3 {
            self.min[k] = self.min[k].min(p[k]);
            self.max[k] = self.max[k].max(p[k]);
        }
    }

    pub fn is_empty(&self) -> bool {
        (0..3).any(|k| self.min[k] > self.max[k])
    }
}

/// What a draw belongs to in the BSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Owner {
    /// A cluster, by index in `sbsp+0x9c`.
    Cluster(u32),
    /// An instanced-geometry placement, by index in `sbsp+0x140`.
    Instance(u32),
}

/// A lightmap page through one palette: the cluster's or instance's
/// render info in the lightmap group (docs/formats/lighting.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LightmapKey {
    /// Image index in the group's lightmap bitmap.
    pub page: u16,
    /// Palette index in the group.
    pub palette: u8,
}

/// One drawn part, placed in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldDraw {
    pub owner: Owner,
    /// Index into the BSP's materials (`sbsp+0xa4`), `u16::MAX` none.
    pub material: u16,
    pub lightmap: Option<LightmapKey>,
    /// Range in [`World::indices`] (absolute vertex indices).
    pub first_index: u32,
    pub index_count: u32,
    pub bounds: Aabb,
    /// The batch it is drawn in ([`World::batches`]).
    pub batch: u32,
}

/// A run of draws sharing material and lightmap: one bind group and one
/// multi-draw over `first_draw .. first_draw + draw_count` slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub material: u16,
    pub lightmap: Option<LightmapKey>,
    pub first_draw: u32,
    pub draw_count: u32,
}

/// A section's drawable input, in its own space.
#[derive(Clone, Debug, Default)]
pub struct SectionInput {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Empty when the section has no lightmap coordinates.
    pub lightmap_uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub parts: Vec<PartInput>,
}

/// One part of a section: its material, whether it is transparent (kind
/// 4, drawn by the transparents, not the lightmap layer) and its
/// triangles over the section's vertices.
#[derive(Clone, Debug, Default)]
pub struct PartInput {
    pub material: u16,
    pub transparent: bool,
    pub triangles: Vec<[u16; 3]>,
}

/// How a section is placed: rigid rotation (rows are where local x, y,
/// z go) with a uniform scale, then a translation. The BSP's instances
/// use exactly this (`InstancedGeometryInstance::transform`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub scale: f32,
    pub basis: [[f32; 3]; 3],
    pub position: [f32; 3],
}

impl Placement {
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        position: [0.0; 3],
    };

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.position;
        for (axis, row) in self.basis.iter().enumerate() {
            for k in 0..3 {
                out[k] += self.scale * p[axis] * row[k];
            }
        }
        out
    }

    pub fn normal(&self, n: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (axis, row) in self.basis.iter().enumerate() {
            for k in 0..3 {
                out[k] += n[axis] * row[k];
            }
        }
        out
    }
}

/// Counts of what was left out while building.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub sections: u32,
    pub transparent_parts: u32,
    /// Triangles with an index past the vertices or a repeated corner.
    pub dropped_triangles: u32,
}

/// The built world.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    /// Sorted by batch.
    pub draws: Vec<WorldDraw>,
    pub batches: Vec<Batch>,
    pub bounds: Option<Aabb>,
    pub stats: BuildStats,
}

/// Collects sections, then sorts their draws into batches.
#[derive(Default)]
pub struct WorldBuilder {
    vertices: Vec<WorldVertex>,
    indices: Vec<u32>,
    draws: Vec<WorldDraw>,
    stats: BuildStats,
}

impl WorldBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a section's opaque parts, placed by `place`, lit by
    /// `lightmap` (ignored when the section has no lightmap coordinates).
    pub fn add_section(
        &mut self,
        owner: Owner,
        section: &SectionInput,
        place: &Placement,
        lightmap: Option<LightmapKey>,
    ) {
        self.stats.sections += 1;
        let n = section.positions.len();
        let lit = section.lightmap_uvs.len() == n;
        let lightmap = lightmap.filter(|_| lit);
        let base = self.vertices.len() as u32;
        for i in 0..n {
            let normal = section.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
            self.vertices.push(WorldVertex {
                position: place.point(section.positions[i]),
                uv: section.uvs.get(i).copied().unwrap_or([0.0; 2]),
                lightmap_uv: if lit { section.lightmap_uvs[i] } else { [0.0; 2] },
                normal: place.normal(normal),
            });
        }
        for part in &section.parts {
            if part.transparent {
                self.stats.transparent_parts += 1;
                continue;
            }
            let first_index = self.indices.len() as u32;
            let mut bounds = Aabb::EMPTY;
            for t in &part.triangles {
                let bad = t.iter().any(|&i| usize::from(i) >= n)
                    || t[0] == t[1]
                    || t[1] == t[2]
                    || t[0] == t[2];
                if bad {
                    self.stats.dropped_triangles += 1;
                    continue;
                }
                for &i in t {
                    let v = base + u32::from(i);
                    bounds.add(self.vertices[v as usize].position);
                    self.indices.push(v);
                }
            }
            let index_count = self.indices.len() as u32 - first_index;
            if index_count == 0 {
                continue;
            }
            self.draws.push(WorldDraw {
                owner,
                material: part.material,
                lightmap,
                first_index,
                index_count,
                bounds,
                batch: 0,
            });
        }
    }

    /// Sorts the draws by (material, lightmap, owner) and groups them.
    pub fn finish(mut self) -> World {
        self.draws
            .sort_by_key(|d| (d.material, d.lightmap, d.owner, d.first_index));
        let mut batches: Vec<Batch> = Vec::new();
        for (i, d) in self.draws.iter_mut().enumerate() {
            match batches.last_mut() {
                Some(b) if b.material == d.material && b.lightmap == d.lightmap => b.draw_count += 1,
                _ => batches.push(Batch {
                    material: d.material,
                    lightmap: d.lightmap,
                    first_draw: i as u32,
                    draw_count: 1,
                }),
            }
            d.batch = batches.len() as u32 - 1;
        }
        let mut bounds = Aabb::EMPTY;
        for d in &self.draws {
            bounds.add(d.bounds.min);
            bounds.add(d.bounds.max);
        }
        World {
            vertices: self.vertices,
            indices: self.indices,
            draws: self.draws,
            batches,
            bounds: (!bounds.is_empty()).then_some(bounds),
            stats: self.stats,
        }
    }
}

impl World {
    /// The lightmap pages the draws use, in a fixed order: index `i` is
    /// layer `i` of the lightmap array.
    pub fn lightmap_keys(&self) -> Vec<LightmapKey> {
        let mut keys: Vec<LightmapKey> = self.batches.iter().filter_map(|b| b.lightmap).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The materials the draws use, ascending.
    pub fn materials(&self) -> Vec<u16> {
        let mut m: Vec<u16> = self.batches.iter().map(|b| b.material).collect();
        m.dedup();
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit quad in the xy plane: two triangles, one part.
    fn quad(material: u16, transparent: bool, lit: bool) -> SectionInput {
        SectionInput {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
            lightmap_uvs: if lit { vec![[0.5; 2]; 4] } else { vec![] },
            normals: vec![[0.0, 0.0, 1.0]; 4],
            parts: vec![PartInput {
                material,
                transparent,
                triangles: vec![[0, 1, 2], [1, 3, 2]],
            }],
        }
    }

    const PAGE: Option<LightmapKey> = Some(LightmapKey { page: 2, palette: 1 });

    #[test]
    fn a_section_becomes_one_draw_per_opaque_part() {
        let mut b = WorldBuilder::new();
        let mut s = quad(3, false, true);
        s.parts.push(PartInput {
            material: 4,
            transparent: true,
            triangles: vec![[0, 1, 2]],
        });
        b.add_section(Owner::Cluster(0), &s, &Placement::IDENTITY, PAGE);
        let w = b.finish();
        assert_eq!(w.vertices.len(), 4);
        assert_eq!(w.indices, vec![0, 1, 2, 1, 3, 2]);
        assert_eq!(w.draws.len(), 1);
        assert_eq!(w.stats.transparent_parts, 1);
        let d = w.draws[0];
        assert_eq!((d.first_index, d.index_count, d.material, d.lightmap), (0, 6, 3, PAGE));
        assert_eq!(d.bounds, Aabb { min: [0.0; 3], max: [1.0, 1.0, 0.0] });
        assert_eq!(w.vertices[3].lightmap_uv, [0.5; 2]);
    }

    #[test]
    fn instances_are_placed_in_world_space() {
        let mut b = WorldBuilder::new();
        // A quarter turn about z (x goes to y), doubled, moved up 10.
        let place = Placement {
            scale: 2.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [0.0, 0.0, 10.0],
        };
        b.add_section(Owner::Instance(7), &quad(0, false, false), &place, None);
        let w = b.finish();
        assert_eq!(w.vertices[1].position, [0.0, 2.0, 10.0]);
        assert_eq!(w.vertices[2].position, [-2.0, 0.0, 10.0]);
        assert_eq!(w.vertices[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(w.draws[0].bounds, Aabb { min: [-2.0, 0.0, 10.0], max: [0.0, 2.0, 10.0] });
        assert_eq!(w.draws[0].owner, Owner::Instance(7));
    }

    #[test]
    fn draws_sort_into_batches_by_material_and_page() {
        let mut b = WorldBuilder::new();
        let other = Some(LightmapKey { page: 0, palette: 0 });
        b.add_section(Owner::Cluster(0), &quad(5, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(1), &quad(1, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(2), &quad(5, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(3), &quad(5, false, true), &Placement::IDENTITY, other);
        let w = b.finish();
        let shape: Vec<_> = w.batches.iter().map(|b| (b.material, b.lightmap, b.first_draw, b.draw_count)).collect();
        assert_eq!(shape, vec![(1, PAGE, 0, 1), (5, other, 1, 1), (5, PAGE, 2, 2)]);
        assert!(w.draws.iter().all(|d| w.batches[d.batch as usize].material == d.material));
        assert_eq!(w.lightmap_keys(), vec![other.unwrap(), PAGE.unwrap()]);
        assert_eq!(w.materials(), vec![1, 5]);
    }

    #[test]
    fn an_unlit_section_drops_its_lightmap_and_bad_triangles_are_counted() {
        let mut b = WorldBuilder::new();
        let mut s = quad(0, false, false);
        s.parts[0].triangles.push([0, 0, 1]);
        s.parts[0].triangles.push([0, 1, 9]);
        b.add_section(Owner::Cluster(0), &s, &Placement::IDENTITY, PAGE);
        let w = b.finish();
        assert_eq!(w.draws[0].lightmap, None);
        assert_eq!(w.draws[0].index_count, 6);
        assert_eq!(w.stats.dropped_triangles, 2);
    }
}
