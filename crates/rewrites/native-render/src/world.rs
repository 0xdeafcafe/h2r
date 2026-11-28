//! The static world as GPU-ready arrays: one vertex buffer, one index
//! buffer, one draw per drawn part in world space, sorted into batches
//! that share a material and a lightmap page. Built once at map load;
//! pure data, no GPU and no disc (`crate::load` fills it from a map).

pub mod light_parts;

use bytemuck::{Pod, Zeroable};

/// One world vertex: what the lightmap pass reads. The first
/// four fields are what stage 1's own program reads; the rest are what the
/// game's world programs read (docs/graphics/native-world.md, "What a
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
    /// `normal`, `binormal`, `tangent` and `lighting` as the stream words
    /// the D3D path binds (`NORMPACKED3`, [`pack_normpacked3`]): the
    /// material programs read `v2..v5` from these and decode them in the
    /// shader as the D3D translation does (`crate::program::INPUTS`).
    pub packed: [u32; 4],
    /// Per-vertex light, stored in RGBA order for a normalized byte fetch.
    pub baked_color: [u8; 4],
}

/// `v` as a `NORMPACKED3` word (signed 11:11:10, x in the low bits): each
/// component times 1023, 1023 and 511, rounded and clamped. For a value
/// decoded from such a word (`tags::geometry::unpack_normpacked3`) this is
/// the word again, but for the field `-1024` (or `-512`), which comes back
/// as `-1023` (`-511`): both decode to `-1`, on the CPU and in the shader.
pub fn pack_normpacked3(v: [f32; 3]) -> u32 {
    let field = |x: f32, max: f32, bits: u32| -> u32 {
        let s = (x * max).round().clamp(-max, max) as i32;
        (s as u32) & ((1 << bits) - 1)
    };
    field(v[0], 1023.0, 11) | field(v[1], 1023.0, 11) << 11 | field(v[2], 511.0, 10) << 22
}

/// The four packed words of a vertex's frame and lighting vector.
fn pack_frame(normal: [f32; 3], binormal: [f32; 3], tangent: [f32; 3], lighting: [f32; 3]) -> [u32; 4] {
    [normal, binormal, tangent, lighting].map(pack_normpacked3)
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

/// What a draw belongs to in the BSP (the scene's type).
pub use frame_scene::Owner;

/// A lightmap page through one palette: the cluster's or instance's
/// render info in the lightmap group (docs/formats/lighting.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LightmapKey {
    /// Image index in the group's lightmap bitmap.
    pub page: u16,
    /// Palette index in the group; `None` (render info palette `-1`) for
    /// a colour page, decoded direct (the converted Halo 1 pages).
    pub palette: Option<u8>,
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
    /// vertex. [`BAKED_COLOR_INPUT`] marks the colour alternative for v6.
    pub inputs: u16,
}

/// Required stream bit for a classified program that takes baked colour
/// at v6. The ordinary v6 stream bit denotes lightmap UVs.
pub const BAKED_COLOR_INPUT: u16 = 1 << 7;

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
        has(s.baked_colors.len()),
    ]
    .iter()
    .enumerate()
    .fold(0, |m, (i, &on)| if on { m | 1 << i } else { m })
}

/// One transparent part (kind 4) of a section, placed in the world: what
/// a sorted transparent entry names by its owner and part index
/// (docs/graphics/native-transparents.md, "Stage 3: transparents"). Not in
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
    /// Vertex registers supplied by this section, as in WorldDraw.
    pub inputs: u16,
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
    /// A baked RGBA colour per vertex when this stream is bound.
    pub baked_colors: Vec<[u8; 4]>,
    /// A bucket descriptor overrides the definition's colour stream.
    pub bucket_colors: bool,
    pub parts: Vec<PartInput>,
    /// Original map ranges and the alternative list interpretation of strips.
    pub source: Option<light_parts::SourceSection>,
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
    /// What a material program reads, index for index with `vertices`:
    /// each section's vertices as its streams hold them, unplaced (a
    /// placement's go through the program's rigid classification with its
    /// rows, [`World::rows`]).
    pub program_vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    /// Original part ranges and the triangles each gathered topology draws.
    pub light_parts: std::sync::Arc<light_parts::Ranges>,
    /// Sorted by batch.
    pub draws: Vec<WorldDraw>,
    pub batches: Vec<Batch>,
    /// The transparent parts, sorted by (owner, part) for
    /// [`World::transparent`].
    pub transparents: Vec<TransparentDraw>,
    pub bounds: Option<Aabb>,
    pub stats: BuildStats,
    /// Each placement's model rows ([`model_rows`]), sorted by placement.
    pub placements: Vec<(u32, ModelRows)>,
    /// Where each added section's vertices are, in the order added: an
    /// editor moves a section's vertices in place with
    /// `GpuWorld::set_section_vertices` (docs/editor/modeling.md).
    pub sections: Vec<SectionRun>,
}

/// One added section's run of [`World::vertices`] and how it was placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionRun {
    pub owner: Owner,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub place: Placement,
    bucket_colors: bool,
    has_baked_colors: bool,
}

impl World {
    /// The run of a section added for `owner`.
    pub fn section(&self, owner: Owner) -> Option<&SectionRun> {
        self.sections.iter().find(|s| s.owner == owner)
    }

    /// Replace the positions and normals of `owner`'s section (in its own
    /// space; placed here as when added) in both vertex lists. Returns
    /// the run, or `None` when `owner` has no section or the counts
    /// differ. Draw bounds are not recomputed.
    pub fn set_section_vertices(
        &mut self,
        owner: Owner,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
    ) -> Option<SectionRun> {
        let run = *self.section(owner)?;
        let n = run.vertex_count as usize;
        if positions.len() != n || normals.is_some_and(|v| v.len() != n) {
            return None;
        }
        let first = run.first_vertex as usize;
        let programs = self.program_vertices.len() == self.vertices.len();
        for i in 0..n {
            let v = &mut self.vertices[first + i];
            v.position = if run.place == Placement::IDENTITY {
                positions[i]
            } else {
                run.place.point(positions[i])
            };
            if let Some(normals) = normals {
                v.normal = run.place.normal(normals[i]);
            }
            v.packed = pack_frame(v.normal, v.binormal, v.tangent, v.lighting);
            if programs {
                let p = &mut self.program_vertices[first + i];
                p.position = positions[i];
                if let Some(normals) = normals {
                    p.normal = normals[i];
                }
                p.packed = pack_frame(p.normal, p.binormal, p.tangent, p.lighting);
            }
        }
        Some(run)
    }

    /// Replace `owner`'s section with `section`, decoded again from the
    /// edited map (docs/editor/realtime-edits.md, "The native refresh"):
    /// every vertex as [`WorldBuilder::add_section`] builds it, placed as
    /// when added, then the bounds of its draws and the world's
    /// recomputed. A placement keeps the lightmap coordinates and
    /// lighting vectors it has, which come from its lightmap bucket and
    /// not its definition. Refused, changing nothing, when the vertex
    /// count differs or a part this world draws has other triangles: the
    /// index buffer is not rewritten, so the host reloads the world.
    pub fn replace_section(&mut self, owner: Owner, section: &SectionInput) -> Result<SectionRun, SectionRefusal> {
        let run = *self.section(owner).ok_or(SectionRefusal::NoSection)?;
        let n = run.vertex_count as usize;
        if section.positions.len() != n {
            return Err(SectionRefusal::VertexCount);
        }
        if !run.bucket_colors && (section.baked_colors.len() == n) != run.has_baked_colors {
            return Err(SectionRefusal::ColorStream);
        }
        let base = run.first_vertex;
        let inputs = section_inputs(section, section.lightmap_uvs.len() == n);
        for (&(_, part), ranges) in self.light_parts.range((owner, 0)..=(owner, u8::MAX)) {
            // Placements keep per-instance lightmap coordinates and lighting.
            let inputs = if matches!(owner, Owner::Instance(_)) {
                let bucket = (1 << 5) | (1 << 6);
                (inputs & !bucket) | (ranges.inputs() & bucket)
            } else { inputs };
            if !ranges.unchanged(part, section, &self.indices, base, inputs) {
                return Err(SectionRefusal::Triangles);
            }
        }
        let ranges = self
            .draws
            .iter()
            .filter(|d| d.owner == owner)
            .map(|d| (d.part, d.first_index, d.index_count))
            .chain(self.transparents.iter().filter(|t| t.owner == owner).map(|t| (t.part, t.first_index, t.index_count)));
        for (part, first, count) in ranges {
            let Some(p) = section.parts.get(usize::from(part)) else {
                return Err(SectionRefusal::Triangles);
            };
            let (kept, _) = part_indices(&p.triangles, n, base);
            if self.indices.get(first as usize..(first + count) as usize) != Some(&kept[..]) {
                return Err(SectionRefusal::Triangles);
            }
        }
        let lit = section.lightmap_uvs.len() == n;
        let keep_lighting = matches!(owner, Owner::Instance(_));
        let programs = self.program_vertices.len() == self.vertices.len();
        let first = base as usize;
        for i in 0..n {
            let (mut placed, mut program) = section_vertex(section, &run.place, lit, i);
            if keep_lighting {
                let old = self.vertices[first + i];
                if run.bucket_colors { placed.baked_color = old.baked_color; }
                placed.lightmap_uv = old.lightmap_uv;
                placed.lighting = old.lighting;
                placed.packed = pack_frame(placed.normal, placed.binormal, placed.tangent, placed.lighting);
                if programs {
                    let old = self.program_vertices[first + i];
                    if run.bucket_colors { program.baked_color = old.baked_color; }
                    program.lightmap_uv = old.lightmap_uv;
                    program.lighting = old.lighting;
                    program.packed = pack_frame(program.normal, program.binormal, program.tangent, program.lighting);
                }
            }
            self.vertices[first + i] = placed;
            if programs {
                self.program_vertices[first + i] = program;
            }
        }
        self.recompute_bounds(owner);
        Ok(run)
    }

    /// Recompute the bounds of `owner`'s draws and transparent parts from
    /// the vertices their indices name, and the world's from the draws,
    /// as [`WorldBuilder::finish`] computes them.
    pub fn recompute_bounds(&mut self, owner: Owner) {
        let (vertices, indices) = (&self.vertices, &self.indices);
        let bounds_of = |first: u32, count: u32| {
            let mut b = Aabb::EMPTY;
            for &v in &indices[first as usize..(first + count) as usize] {
                b.add(vertices[v as usize].position);
            }
            b
        };
        for d in self.draws.iter_mut().filter(|d| d.owner == owner) {
            d.bounds = bounds_of(d.first_index, d.index_count);
        }
        for t in self.transparents.iter_mut().filter(|t| t.owner == owner) {
            t.bounds = bounds_of(t.first_index, t.index_count);
        }
        let mut all = Aabb::EMPTY;
        for d in &self.draws {
            all.add(d.bounds.min);
            all.add(d.bounds.max);
        }
        self.bounds = (!all.is_empty()).then_some(all);
    }
}

/// Why [`World::replace_section`] refused a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionRefusal {
    /// The world has no section for the owner (it did not decode at load).
    NoSection,
    /// The vertex count changed.
    VertexCount,
    /// Rebuild classifications when a section colour stream appears or disappears.
    ColorStream,
    /// A part the world draws has other triangles, or is gone.
    Triangles,
}

/// The three model rows `c[50..52]` (hardware numbering) the structure
/// draw uploads for a placement.
pub type ModelRows = [[f32; 4]; 3];

/// The bytes of a placement (`sbsp+0x144`, `0x58` each) [`model_rows`]
/// reads: through the position at `+0x28`.
pub const PLACEMENT_ROW_BYTES: usize = 0x34;

/// The model rows the structure draw uploads for the placement whose tag
/// element is `element` (`0x0004d830`, the render record's `+0x1c`
/// pointing at it): `c[50+k]` is component `k` of forward (`+4`), left
/// (`+0x10`), up (`+0x1c`) and the position (`+0x28`), copied bit for bit
/// (`movss` loads and stores), the scale at `+0` not applied. `None` when
/// `element` is short.
pub fn model_rows(element: &[u8]) -> Option<ModelRows> {
    let f = |o: usize| -> Option<f32> { Some(f32::from_bits(u32::from_le_bytes(element.get(o..o + 4)?.try_into().ok()?))) };
    let mut rows = [[0.0; 4]; 3];
    for (k, row) in rows.iter_mut().enumerate() {
        for (c, at) in [4, 0x10, 0x1c, 0x28].into_iter().enumerate() {
            row[c] = f(at + 4 * k)?;
        }
    }
    Some(rows)
}

/// A part's triangles as absolute indices from `base`, the ones with an
/// index past the section's `n` vertices or a repeated corner left out,
/// and how many were.
fn part_indices(triangles: &[[u16; 3]], n: usize, base: u32) -> (Vec<u32>, u32) {
    let mut kept = Vec::with_capacity(triangles.len() * 3);
    let mut dropped = 0;
    for t in triangles {
        let bad = t.iter().any(|&i| usize::from(i) >= n) || t[0] == t[1] || t[1] == t[2] || t[0] == t[2];
        if bad {
            dropped += 1;
            continue;
        }
        kept.extend(t.iter().map(|&i| base + u32::from(i)));
    }
    (kept, dropped)
}

/// Vertex `i` of `section` placed by `place` (what the lightmap pass and
/// culling read) and as its streams hold it (what a material program
/// reads), `lit` when the section has lightmap coordinates for every
/// vertex. One rule for a load and a live edit's replacement
/// ([`World::replace_section`]).
fn section_vertex(section: &SectionInput, place: &Placement, lit: bool, i: usize) -> (WorldVertex, WorldVertex) {
    let normal = section.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
    let turn = |v: &Vec<[f32; 3]>| v.get(i).map_or([0.0; 3], |&v| place.normal(v));
    let own = |v: &Vec<[f32; 3]>| v.get(i).copied().unwrap_or([0.0; 3]);
    let (binormal, tangent, lighting) = (own(&section.binormals), own(&section.tangents), own(&section.lighting));
    let uv = section.uvs.get(i).copied().unwrap_or([0.0; 2]);
    let lightmap_uv = if lit { section.lightmap_uvs[i] } else { [0.0; 2] };
    let baked_color = section.baked_colors.get(i).copied().unwrap_or_default();
    let program = WorldVertex {
        position: section.positions[i],
        uv,
        lightmap_uv,
        normal,
        binormal,
        tangent,
        lighting,
        packed: pack_frame(normal, binormal, tangent, lighting),
        baked_color,
    };
    let (placed_normal, placed_binormal, placed_tangent, placed_lighting) = (
        place.normal(normal),
        turn(&section.binormals),
        turn(&section.tangents),
        turn(&section.lighting),
    );
    let placed = WorldVertex {
        // Unplaced geometry (a cluster) keeps its positions' bits: the D3D
        // path feeds them to the same program, and the passes after layer
        // 3 test depth equal against what it writes.
        position: if *place == Placement::IDENTITY {
            section.positions[i]
        } else {
            place.point(section.positions[i])
        },
        uv,
        lightmap_uv,
        normal: placed_normal,
        binormal: placed_binormal,
        tangent: placed_tangent,
        // S: in the section's own space like the tangent frame, so it
        // turns with the placement (the game's program dots the two
        // together).
        lighting: placed_lighting,
        packed: pack_frame(placed_normal, placed_binormal, placed_tangent, placed_lighting),
        baked_color,
    };
    (placed, program)
}

/// Collects sections, then sorts their draws into batches.
#[derive(Default)]
pub struct WorldBuilder {
    vertices: Vec<WorldVertex>,
    program_vertices: Vec<WorldVertex>,
    placements: Vec<(u32, ModelRows)>,
    indices: Vec<u32>,
    light_parts: light_parts::Ranges,
    draws: Vec<WorldDraw>,
    transparents: Vec<TransparentDraw>,
    stats: BuildStats,
    sections: Vec<SectionRun>,
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
        self.sections.push(SectionRun {
            owner,
            first_vertex: base,
            vertex_count: n as u32,
            place: *place,
            bucket_colors: section.bucket_colors,
            has_baked_colors: inputs & BAKED_COLOR_INPUT != 0,
        });
        for i in 0..n {
            let (placed, program) = section_vertex(section, place, lit, i);
            self.program_vertices.push(program);
            self.vertices.push(placed);
        }
        for (index, part) in section.parts.iter().enumerate() {
            if part.transparent {
                self.stats.transparent_parts += 1;
            }
            let first_index = self.indices.len() as u32;
            let (kept, dropped) = part_indices(&part.triangles, n, base);
            self.stats.dropped_triangles += dropped;
            let mut bounds = Aabb::EMPTY;
            for &v in &kept {
                bounds.add(self.vertices[v as usize].position);
            }
            self.indices.extend(kept);
            let index_count = self.indices.len() as u32 - first_index;
            if index_count == 0 {
                continue;
            }
            let Ok(part_index) = u8::try_from(index) else { continue };
            if let Some(source) = &section.source
                && let Some(ranges) = source.append(part_index, &mut self.indices,
                    first_index..first_index + index_count, n, base, part.material, inputs)
            {
                self.light_parts.insert((owner, part_index), ranges);
            }
            if part.transparent {
                self.transparents.push(TransparentDraw {
                    owner,
                    part: part_index,
                    material: part.material,
                    lightmap,
                    first_index,
                    index_count,
                    bounds,
                    inputs,
                });
                continue;
            }
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

    /// Placement `instance`'s model rows, for a material program's rigid
    /// classification ([`World::rows`]).
    pub fn place_rows(&mut self, instance: u32, rows: ModelRows) {
        self.placements.push((instance, rows));
    }

    /// Sorts the draws by (material, lightmap, owner) and groups them.
    pub fn finish(mut self) -> World {
        let mut batches = Vec::new();
        sort_and_batch(&mut self.draws, &mut batches);
        let mut bounds = Aabb::EMPTY;
        for d in &self.draws {
            bounds.add(d.bounds.min);
            bounds.add(d.bounds.max);
        }
        self.transparents.sort_by_key(|t| (t.owner, t.part));
        self.placements.sort_by_key(|p| p.0);
        self.placements.dedup_by_key(|p| p.0);
        World {
            vertices: self.vertices,
            program_vertices: self.program_vertices,
            placements: self.placements,
            indices: self.indices,
            light_parts: std::sync::Arc::new(self.light_parts),
            draws: self.draws,
            batches,
            transparents: self.transparents,
            bounds: (!bounds.is_empty()).then_some(bounds),
            stats: self.stats,
            sections: self.sections,
        }
    }
}

/// `world` with only the draws `keep` accepts, its batches rebuilt over
/// them: one per run of equal material and lightmap, each draw's `batch`
/// renumbered. A host leaves out the draws it has no texture for (the
/// game, the editor, Theater's host view).
pub fn keep_draws(mut world: World, keep: &dyn Fn(&WorldDraw) -> bool) -> World {
    world.draws.retain(|d| keep(d));
    rebuild_batches(&mut world.draws, &mut world.batches);
    world
}

fn sort_and_batch(draws: &mut [WorldDraw], batches: &mut Vec<Batch>) {
    draws.sort_by_key(|d| (d.material, d.lightmap, d.owner, d.first_index));
    rebuild_batches(draws, batches);
}

fn rebuild_batches(draws: &mut [WorldDraw], batches: &mut Vec<Batch>) {
    batches.clear();
    for (i, d) in draws.iter_mut().enumerate() {
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
}

impl World {
    /// Regroup the resident layer-3 draws after assigning a program slot
    /// to each (BSP material, lighting implementation) pair.
    pub fn regroup_draws(&mut self) {
        sort_and_batch(&mut self.draws, &mut self.batches);
    }

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

    /// Placement `instance`'s model rows, when the load gave them.
    pub fn rows(&self, instance: u32) -> Option<&ModelRows> {
        self.placements
            .binary_search_by_key(&instance, |p| p.0)
            .ok()
            .map(|i| &self.placements[i].1)
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

    const PAGE: Option<LightmapKey> = Some(LightmapKey { page: 2, palette: Some(1) });

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

    /// An editor's moved vertices land on the section's own run, placed
    /// as it was added; the other section keeps its vertices.
    #[test]
    fn a_sections_vertices_are_replaced_in_place() {
        let place = Placement {
            scale: 1.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [10.0, 0.0, 0.0],
        };
        let mut b = WorldBuilder::new();
        b.add_section(Owner::Cluster(0), &quad(0, false, false), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(3), &quad(0, false, false), &place, None);
        let mut w = b.finish();
        assert_eq!(w.sections.len(), 2);
        let before = w.vertices[..4].to_vec();
        let moved = [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];
        let run = w.set_section_vertices(Owner::Instance(3), &moved, None).unwrap();
        assert_eq!((run.first_vertex, run.vertex_count), (4, 4));
        assert_eq!(w.vertices[4].position, [10.0, 0.0, 1.0]);
        assert_eq!(w.program_vertices[4].position, [0.0, 0.0, 1.0]);
        assert_eq!(w.vertices[5].position, [10.0, 1.0, 0.0], "placed as added");
        assert_eq!(w.vertices[..4], before[..]);
        assert!(w.set_section_vertices(Owner::Cluster(0), &moved[..3], None).is_none());
        assert!(w.set_section_vertices(Owner::Cluster(9), &moved, None).is_none());
    }

    /// A live edit's replacement leaves the world equal to one built from
    /// the edited section: vertices, draw bounds and the world's bounds.
    /// A placement keeps its bucket's lightmap coordinates; a changed
    /// vertex count or triangle list is refused and changes nothing.
    #[test]
    fn a_replaced_section_equals_a_world_built_from_it() {
        let place = Placement {
            scale: 1.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [10.0, 0.0, 0.0],
        };
        let build = |cluster: &SectionInput, instance: &SectionInput| {
            let mut b = WorldBuilder::new();
            b.add_section(Owner::Cluster(0), cluster, &Placement::IDENTITY, PAGE);
            b.add_section(Owner::Instance(3), instance, &place, PAGE);
            b.finish()
        };
        let mut edited = quad(0, false, true);
        edited.positions[3] = [4.0, 5.0, 6.0];
        edited.uvs[3] = [0.25, 0.75];
        let mut w = build(&quad(0, false, true), &quad(0, false, true));
        let want = build(&edited, &quad(0, false, true));
        assert!(w.replace_section(Owner::Cluster(0), &edited).is_ok());
        assert_eq!(w.vertices, want.vertices);
        assert_eq!(w.program_vertices, want.program_vertices);
        assert_eq!(w.draws, want.draws, "bounds recomputed");
        assert_eq!(w.bounds, want.bounds);

        // The placement's bucket coordinates stay; its definition's do not
        // replace them.
        let mut own = quad(0, false, true);
        own.lightmap_uvs = vec![[0.125; 2]; 4];
        own.baked_colors = vec![[12, 21, 42, 129]; 4];
        own.bucket_colors = true;
        let mut w = build(&quad(0, false, true), &own);
        let mut def = edited.clone();
        def.lightmap_uvs = vec![[0.9; 2]; 4];
        let mut moved = edited.clone();
        moved.lightmap_uvs = own.lightmap_uvs.clone();
        moved.baked_colors = own.baked_colors.clone();
        let want = build(&quad(0, false, true), &moved);
        assert!(w.replace_section(Owner::Instance(3), &def).is_ok());
        assert_eq!(w.vertices, want.vertices);
        assert_eq!(w.draws, want.draws);

        let mut from_section = own.clone();
        from_section.bucket_colors = false;
        let mut edited_colors = build(&quad(0, false, true), &from_section);
        assert_eq!(edited_colors.replace_section(Owner::Instance(3), &def), Err(SectionRefusal::ColorStream));
        def.baked_colors = vec![[7, 11, 13, 17]; 4];
        assert!(edited_colors.replace_section(Owner::Instance(3), &def).is_ok());
        assert_eq!(edited_colors.program_vertices[4].baked_color, [7, 11, 13, 17]);
        let before = w.clone();
        let mut fewer = quad(0, false, true);
        fewer.positions.pop();
        assert_eq!(w.replace_section(Owner::Cluster(0), &fewer), Err(SectionRefusal::VertexCount));
        let mut turned = quad(0, false, true);
        turned.parts[0].triangles = vec![[0, 2, 1], [1, 3, 2]];
        assert_eq!(w.replace_section(Owner::Cluster(0), &turned), Err(SectionRefusal::Triangles));
        assert_eq!(w.replace_section(Owner::Cluster(7), &edited), Err(SectionRefusal::NoSection));
        assert_eq!(w.vertices, before.vertices);
        assert_eq!(w.draws, before.draws);
    }

    /// Dropping a material's draws leaves one batch per run of what is
    /// left, and each draw names its new batch.
    #[test]
    fn keeping_some_draws_rebuilds_the_batches_over_them() {
        let mut b = WorldBuilder::new();
        for (i, m) in [1u16, 2, 1].into_iter().enumerate() {
            b.add_section(Owner::Cluster(i as _), &quad(m, false, true), &Placement::IDENTITY, PAGE);
        }
        let all = b.finish();
        assert_eq!(all.draws.len(), 3);
        let kept = keep_draws(all.clone(), &|d| d.material == 1);
        assert_eq!(kept.draws.len(), 2);
        assert!(kept.draws.iter().all(|d| d.material == 1 && d.batch == 0));
        assert_eq!(
            kept.batches,
            vec![Batch { material: 1, lightmap: PAGE, first_draw: 0, draw_count: 2 }]
        );
        let none = keep_draws(all, &|_| false);
        assert!(none.draws.is_empty() && none.batches.is_empty());
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
    fn a_cluster_keeps_its_positions_bit_for_bit() {
        // The D3D path feeds a cluster's positions to the world program
        // unchanged, and the passes after layer 3 (shadows, lights, fog,
        // overlays) test depth equal against what layer 3 wrote: the
        // native pass must feed the same bits. Identity arithmetic would
        // not (0 + -0 is +0; x + 0·y is not x once y is infinite).
        let mut b = WorldBuilder::new();
        let mut s = quad(0, false, false);
        s.positions[0] = [-0.0, f32::from_bits(1), 1.0e30];
        s.positions[1] = [0.1, -1.0e-30, f32::INFINITY];
        b.add_section(Owner::Cluster(0), &s, &Placement::IDENTITY, None);
        let w = b.finish();
        let bits = |p: [f32; 3]| p.map(f32::to_bits);
        for i in 0..s.positions.len() {
            assert_eq!(bits(w.vertices[i].position), bits(s.positions[i]), "vertex {i}");
        }
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
    fn a_program_reads_every_section_unplaced() {
        // A placement's vertices go through the rigid classification,
        // which places them on the GPU with its rows: the program's copy
        // holds the definition's streams as they are, while the placed
        // copy (bounds, culling, stage 1) is in world space.
        let mut b = WorldBuilder::new();
        let place = Placement {
            scale: 2.0,
            basis: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            position: [0.0, 0.0, 10.0],
        };
        let mut s = quad(0, false, true);
        s.positions[3] = [-0.0, f32::from_bits(1), 1.0e30];
        s.normals[1] = [1.0, 0.0, 0.0];
        s.lighting = vec![[0.25, -0.5, 1.0]; 4];
        b.add_section(Owner::Cluster(0), &quad(1, false, false), &Placement::IDENTITY, None);
        b.add_section(Owner::Instance(7), &s, &place, PAGE);
        b.place_rows(7, [[1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0], [9.0, 10.0, 11.0, 12.0]]);
        let w = b.finish();
        assert_eq!(w.program_vertices.len(), w.vertices.len());
        let bits = |p: [f32; 3]| p.map(f32::to_bits);
        for i in 0..4 {
            let v = w.program_vertices[4 + i];
            assert_eq!(bits(v.position), bits(s.positions[i]), "vertex {i}");
            assert_eq!(v.normal, s.normals[i]);
            assert_eq!(v.lighting, s.lighting[i]);
            assert_eq!(v.lightmap_uv, [0.5; 2]);
        }
        assert_eq!(w.vertices[5].position, [0.0, 2.0, 10.0], "the placed copy");
        assert_eq!(w.vertices[5].normal, [0.0, 1.0, 0.0]);
        assert_eq!(w.program_vertices[1], w.vertices[1], "a cluster's copies agree");
        assert_eq!(w.rows(7).map(|r| r[1][3]), Some(8.0));
        assert_eq!(w.rows(0), None);
        // The programs' words: the section's own frame and lighting vector.
        let v = w.program_vertices[5];
        assert_eq!(v.packed[0], pack_normpacked3([1.0, 0.0, 0.0]));
        assert_eq!(v.packed[3], pack_normpacked3([0.25, -0.5, 1.0]));
    }

    /// The words a vertex program reads `v2..v5` from (the round trip
    /// through the decode is `load::tests`).
    #[test]
    fn a_frame_packs_into_normpacked3_words() {
        assert_eq!(pack_normpacked3([1.0, -1.0, 0.5]), 0x3ff | 0x401 << 11 | 256 << 22);
        assert_eq!(pack_normpacked3([3.0 / 1023.0, -2.0 / 1023.0, -1.0 / 511.0]), 3 | 0x7fe << 11 | 0x3ff << 22);
        // Out of range and the default normal.
        assert_eq!(pack_normpacked3([2.0, -2.0, 0.0]), 0x3ff | 0x401 << 11);
        assert_eq!(pack_normpacked3([0.0, 0.0, 1.0]), 0x1ff << 22);
        let b = {
            let mut b = WorldBuilder::new();
            let mut s = quad(0, false, true);
            s.normals.clear();
            b.add_section(Owner::Cluster(0), &s, &Placement::IDENTITY, None);
            b.finish()
        };
        assert_eq!(b.program_vertices[0].packed, [0x1ff << 22, 0, 0, 0], "no normals: +z; nothing else: zero");
    }

    #[test]
    fn model_rows_are_the_basis_columns_and_the_position() {
        // `0x0004d830`: c[50] = (forward.x, left.x, up.x, position.x), and
        // so on; the scale (+0) is not applied, and the bits are copied.
        let mut e = vec![0u8; 0x58];
        let words: [u32; 13] = [
            2.0f32.to_bits(),
            1.0f32.to_bits(),
            0.5f32.to_bits(),
            0x7fc0_1234,
            (-0.0f32).to_bits(),
            3.0f32.to_bits(),
            4.0f32.to_bits(),
            5.0f32.to_bits(),
            6.0f32.to_bits(),
            7.0f32.to_bits(),
            100.0f32.to_bits(),
            f32::INFINITY.to_bits(),
            1,
        ];
        for (i, w) in words.iter().enumerate() {
            e[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        let rows = model_rows(&e).unwrap();
        let bits = rows.map(|r| r.map(f32::to_bits));
        assert_eq!(bits[0], [words[1], words[4], words[7], words[10]]);
        assert_eq!(bits[1], [words[2], words[5], words[8], words[11]]);
        assert_eq!(bits[2], [words[3], words[6], words[9], words[12]]);
        assert_eq!(model_rows(&e[..PLACEMENT_ROW_BYTES]).map(|r| r.map(|r| r.map(f32::to_bits))), Some(bits));
        assert_eq!(model_rows(&e[..PLACEMENT_ROW_BYTES - 1]), None);
    }

    #[test]
    fn draws_sort_into_batches_by_material_and_page() {
        let mut b = WorldBuilder::new();
        let other = LightmapKey { page: 0, palette: Some(0) };
        b.add_section(Owner::Cluster(0), &quad(5, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(1), &quad(1, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(Owner::Cluster(2), &quad(5, false, true), &Placement::IDENTITY, PAGE);
        b.add_section(
            Owner::Cluster(3),
            &quad(5, false, true),
            &Placement::IDENTITY,
            Some(other),
        );
        let w = b.finish();
        let shape: Vec<_> = w.batches.iter().map(|b| (b.material, b.lightmap, b.first_draw, b.draw_count)).collect();
        assert_eq!(
            shape,
            vec![(1, PAGE, 0, 1), (5, Some(other), 1, 1), (5, PAGE, 2, 2)]
        );
        assert!(w.draws.iter().all(|d| w.batches[d.batch as usize].material == d.material));
        assert_eq!(w.lightmap_keys(), vec![other, PAGE.unwrap()]);
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
