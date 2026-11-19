//! The static world as GPU-ready arrays: one vertex buffer, one index
//! buffer, one draw per drawn part in world space, sorted into batches
//! that share a material and a lightmap page. Built once at map load;
//! pure data, no GPU and no disc (`crate::load` fills it from a map).

use bytemuck::{Pod, Zeroable};

/// One world vertex (76 bytes): what the lightmap pass reads. The first
/// four fields are what stage 1's own program reads; the rest are what the
/// game's world programs read (docs/graphics/native-renderer.md, "What a
/// world pass reads"): `v2..v4` the tangent frame, `v5` the lighting
/// vector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WorldVertex {
    pub position: [f32; 3],
    /// Base-map coordinates (stream texcoord).
    pub uv: [f32; 2],
    /// Lightmap page coordinates (stream `1f`), zero without a lightmap.
    pub lightmap_uv: [f32; 2],
    pub normal: [f32; 3],
    pub binormal: [f32; 3],
    pub tangent: [f32; 3],
    /// The lightmap's signed lighting vector (stream `30`), zero without one.
    pub lighting: [f32; 3],
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

    /// Whether the box and the sphere (`centre`, `radius`) share a point;
    /// an empty box meets nothing.
    pub fn meets_sphere(&self, centre: [f32; 3], radius: f32) -> bool {
        if self.is_empty() || radius.is_nan() || radius < 0.0 {
            return false;
        }
        let d2: f32 = (0..3)
            .map(|k| {
                let d = (self.min[k] - centre[k]).max(0.0).max(centre[k] - self.max[k]);
                d * d
            })
            .sum();
        d2 <= radius * radius
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
    /// The part's index in its section (transparent and undrawn parts
    /// counted too): the low byte of a layer item's render argument,
    /// which the build `0x0004cbf0` uses as the index into the section's
    /// parts (`[section+4] + 0x48·part`).
    pub part: u8,
    /// Index into the BSP's materials (`sbsp+0xa4`), `u16::MAX` none.
    pub material: u16,
    pub lightmap: Option<LightmapKey>,
    /// Range in [`World::indices`] (absolute vertex indices).
    pub first_index: u32,
    pub index_count: u32,
    pub bounds: Aabb,
    /// The batch it is drawn in ([`World::batches`]).
    pub batch: u32,
    /// The vertex registers its section supplies, bit `n` for `v<n>` as a
    /// material program reads them (`crate::program::INPUTS`): `v0` the
    /// position always, `v1` the texture coordinate, `v2..v4` the
    /// normal, binormal and tangent, `v5` the lighting vector, `v6` the
    /// lightmap coordinate; each only when the section has it for every
    /// vertex (the rest read zero, or the default normal).
    pub inputs: u16,
}

/// [`WorldDraw::inputs`] of a section with `n` vertices.
fn section_inputs(s: &SectionInput, lit: bool) -> u16 {
    let n = s.positions.len();
    let has = |len: usize| len == n;
    [
        true,
        has(s.uvs.len()),
        has(s.normals.len()),
        has(s.binormals.len()),
        has(s.tangents.len()),
        has(s.lighting.len()),
        lit,
    ]
    .iter()
    .enumerate()
    .fold(0, |m, (i, &on)| if on { m | 1 << i } else { m })
}

/// One transparent part (kind 4) of a section, placed in the world: what
/// a sorted transparent entry names by its owner and part index
/// (docs/graphics/native-renderer.md, "Stage 3: transparents"). Not in
/// any batch: the transparents draw them one entry at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransparentDraw {
    pub owner: Owner,
    /// The part's index in its section (opaque parts counted too): the low
    /// byte of the entry's render argument (S: the build `0x0004cbf0`
    /// reads it as the section's part).
    pub part: u8,
    /// Index into the BSP's materials (`sbsp+0xa4`), `u16::MAX` none.
    pub material: u16,
    pub lightmap: Option<LightmapKey>,
    /// Range in [`World::indices`] (absolute vertex indices).
    pub first_index: u32,
    pub index_count: u32,
    pub bounds: Aabb,
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
    /// Empty when the section has no tangent frame.
    pub binormals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    /// The signed lighting vectors (stream `30`); empty without them.
    pub lighting: Vec<[f32; 3]>,
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
    /// The transparent parts, sorted by (owner, part) for
    /// [`World::transparent`].
    pub transparents: Vec<TransparentDraw>,
    pub bounds: Option<Aabb>,
    pub stats: BuildStats,
}

/// Collects sections, then sorts their draws into batches.
#[derive(Default)]
pub struct WorldBuilder {
    vertices: Vec<WorldVertex>,
    indices: Vec<u32>,
    draws: Vec<WorldDraw>,
    transparents: Vec<TransparentDraw>,
    stats: BuildStats,
}

impl WorldBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a section's parts, placed by `place`, lit by `lightmap`
    /// (ignored when the section has no lightmap coordinates): the opaque
    /// ones as batched draws, the transparent ones as [`TransparentDraw`]s.
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
        let inputs = section_inputs(section, lit);
        let base = self.vertices.len() as u32;
        for i in 0..n {
            let normal = section.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
            let turn = |v: &Vec<[f32; 3]>| v.get(i).map_or([0.0; 3], |&v| place.normal(v));
            self.vertices.push(WorldVertex {
                position: place.point(section.positions[i]),
                uv: section.uvs.get(i).copied().unwrap_or([0.0; 2]),
                lightmap_uv: if lit { section.lightmap_uvs[i] } else { [0.0; 2] },
                normal: place.normal(normal),
                binormal: turn(&section.binormals),
                tangent: turn(&section.tangents),
                // S: in the section's own space like the tangent frame, so
                // it turns with the placement (the game's program dots the
                // two together).
                lighting: turn(&section.lighting),
            });
        }
        for (index, part) in section.parts.iter().enumerate() {
            if part.transparent {
                self.stats.transparent_parts += 1;
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
            if part.transparent {
                // Past 255 parts the entry's byte cannot name it.
                if let Ok(part_index) = u8::try_from(index) {
                    self.transparents.push(TransparentDraw {
                        owner,
                        part: part_index,
                        material: part.material,
                        lightmap,
                        first_index,
                        index_count,
                        bounds,
                    });
                }
                continue;
            }
            // Past 255 parts an item's byte cannot name it.
            let Ok(part_index) = u8::try_from(index) else { continue };
            self.draws.push(WorldDraw {
                owner,
                part: part_index,
                material: part.material,
                lightmap,
                first_index,
                index_count,
                bounds,
                batch: 0,
                inputs,
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
        self.transparents.sort_by_key(|t| (t.owner, t.part));
        World {
            vertices: self.vertices,
            indices: self.indices,
            draws: self.draws,
            batches,
            transparents: self.transparents,
            bounds: (!bounds.is_empty()).then_some(bounds),
            stats: self.stats,
        }
    }
}

impl World {
    /// The lightmap pages the draws and the transparent parts use, in a
    /// fixed order: index `i` is layer `i` of the lightmap array.
    pub fn lightmap_keys(&self) -> Vec<LightmapKey> {
        let mut keys: Vec<LightmapKey> = self
            .batches
            .iter()
            .filter_map(|b| b.lightmap)
            .chain(self.transparents.iter().filter_map(|t| t.lightmap))
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The transparent part `part` of `owner`'s section.
    pub fn transparent(&self, owner: Owner, part: u8) -> Option<&TransparentDraw> {
        self.transparents
            .binary_search_by_key(&(owner, part), |t| (t.owner, t.part))
            .ok()
            .map(|i| &self.transparents[i])
    }

    /// The materials the transparent parts use, ascending, deduplicated.
    pub fn transparent_materials(&self) -> Vec<u16> {
        let mut m: Vec<u16> = self.transparents.iter().map(|t| t.material).collect();
        m.sort_unstable();
        m.dedup();
        m
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

    #[test]
    fn a_box_meets_a_sphere_that_reaches_it() {
        let b = Aabb { min: [0.0; 3], max: [1.0; 3] };
        assert!(b.meets_sphere([0.5; 3], 0.1), "inside");
        assert!(b.meets_sphere([2.0, 0.5, 0.5], 1.0), "touching a face");
        assert!(!b.meets_sphere([2.0, 0.5, 0.5], 0.99));
        assert!(b.meets_sphere([2.0, 2.0, 0.5], 1.5), "past an edge");
        assert!(!b.meets_sphere([2.0, 2.0, 2.0], 1.7), "corner at sqrt(3)");
        assert!(b.meets_sphere([2.0, 2.0, 2.0], 1.733));
        assert!(!Aabb::EMPTY.meets_sphere([0.0; 3], 1e9));
        assert!(!b.meets_sphere([0.5; 3], f32::NAN));
    }

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
            ..Default::default()
        }
    }

    const PAGE: Option<LightmapKey> = Some(LightmapKey { page: 2, palette: 1 });

    #[test]
    fn the_tangent_frame_and_lighting_vector_turn_with_the_placement() {
        let mut s = quad(0, false, true);
        s.binormals = vec![[1.0, 0.0, 0.0]; 4];
        s.tangents = vec![[0.0, 1.0, 0.0]; 4];
        s.lighting = vec![[0.5, 0.0, 0.0]; 4];
        // A quarter turn about z: local x goes to y, y to -x.
        let place = Placement {
            scale: 1.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [0.0; 3],
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Instance(0), &s, &place, PAGE);
        let v = b.finish().vertices[0];
        assert_eq!(v.binormal, [0.0, 1.0, 0.0]);
        assert_eq!(v.tangent, [-1.0, 0.0, 0.0]);
        assert_eq!(v.lighting, [0.0, 0.5, 0.0], "turned, length kept");
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &quad(0, false, true), &Placement::IDENTITY, PAGE);
        let v = b.finish().vertices[0];
        assert_eq!((v.binormal, v.tangent, v.lighting), ([0.0; 3], [0.0; 3], [0.0; 3]), "none: zero");
    }

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
        assert_eq!(w.indices, vec![0, 1, 2, 1, 3, 2, 0, 1, 2], "the transparent part's after");
        assert_eq!(w.draws.len(), 1);
        assert_eq!(w.stats.transparent_parts, 1);
        let t = w.transparent(Owner::Cluster(0), 1).expect("part 1");
        assert_eq!((t.first_index, t.index_count, t.material, t.lightmap), (6, 3, 4, PAGE));
        assert_eq!(w.transparent(Owner::Cluster(0), 0), None, "part 0 is opaque");
        assert_eq!(w.transparent_materials(), vec![4]);
        assert!(!w.batches.iter().any(|b| b.material == 4), "not batched");
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
    fn a_draw_says_which_vertex_registers_its_section_supplies() {
        let mut full = quad(0, false, true);
        full.binormals = vec![[1.0, 0.0, 0.0]; 4];
        full.tangents = vec![[0.0, 1.0, 0.0]; 4];
        full.lighting = vec![[0.0, 0.0, 1.0]; 4];
        let mut short = full.clone();
        short.lighting.pop();
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &full, &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(1), &short, &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(2), &quad(0, false, false), &Placement::IDENTITY, PAGE);
        let w = b.finish();
        let inputs: Vec<_> = w.draws.iter().map(|d| (d.owner, d.inputs)).collect();
        assert_eq!(
            inputs,
            vec![
                (Owner::Cluster(2), 0b000_0111),
                (Owner::Cluster(0), 0b111_1111),
                (Owner::Cluster(1), 0b101_1111),
            ],
            "unlit with no frame: v0..v2 (sorted first, no page); every register; a short lighting stream is none (v5)"
        );
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
