//! What the viewport draws and how the mouse maps into the world: the
//! structure BSP as triangles, a marker per placement, the camera, picking
//! and plane dragging. All coordinates are game units with z up.
//!
//! The BSP walk is the same as `cli::live_geometry::bsp_triangles` (clusters
//! as stored, instanced geometry placed); it is repeated here in a few lines
//! rather than pulling the CLI's dependencies into the editor.
use crate::layout::{BlockKind, Role};
use crate::save::MapSource;
use crate::scenario::{Scenario, Value};
use map::{Map, RawPointer, ResourceFile, TagId};
use std::io::{Read, Seek};
use tags::geometry::{RawSource, Section, TagView};
use tags::structure::{Lightmap, LightmapRenderInfo, StructureRender};

pub type Vec3 = [f32; 3];
pub type Mat4 = [[f32; 4]; 4];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub fn normalize(a: Vec3) -> Vec3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// One BSP triangle with a face normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tri {
    pub p: [Vec3; 3],
    pub n: Vec3,
}

/// Axis-aligned bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}
impl Bounds {
    pub const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    pub fn add(&mut self, p: Vec3) {
        for (k, v) in p.into_iter().enumerate() {
            self.min[k] = self.min[k].min(v);
            self.max[k] = self.max[k].max(v);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
    pub fn center(&self) -> Vec3 {
        scale(add(self.min, self.max), 0.5)
    }
    pub fn radius(&self) -> f32 {
        dot(sub(self.max, self.min), sub(self.max, self.min)).sqrt() * 0.5
    }
}

/// Reads raw section data from the map itself and its shared caches.
pub struct RawFiles<R> {
    files: Vec<(ResourceFile, Map<R>)>,
}
impl<R: Read + Seek> RawFiles<R> {
    pub fn new(files: Vec<(ResourceFile, Map<R>)>) -> Self {
        Self { files }
    }
}
impl<R: Read + Seek> RawSource for RawFiles<R> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let (_, map) = self
            .files
            .iter_mut()
            .find(|(f, _)| *f == p.file)
            .ok_or(tags::Error::MissingResource(p.file))?;
        Ok(map.read_file_range(p.offset.into(), size.into())?)
    }
}

/// Open the shared caches next to a map: siblings in the same directory for
/// a file, other maps on the same disc for a disc map. Missing ones are
/// skipped; sections that need them then fail to load and are counted.
pub fn shared_sources(source: &MapSource) -> Vec<(ResourceFile, MapSource)> {
    let names = [
        (ResourceFile::MainMenu, "mainmenu"),
        (ResourceFile::Shared, "shared"),
        (ResourceFile::SinglePlayerShared, "single_player_shared"),
    ];
    match source.region {
        Some(_) => {
            let maps = MapSource::disc_maps(&source.path).unwrap_or_default();
            names
                .iter()
                .filter_map(|(f, n)| {
                    let (_, s) = maps.iter().find(|(m, _)| m.eq_ignore_ascii_case(n))?;
                    Some((*f, s.clone()))
                })
                .collect()
        }
        None => names
            .iter()
            .filter_map(|(f, n)| {
                let p = source.path.with_file_name(format!("{n}.map"));
                p.is_file().then(|| (*f, MapSource::file(p)))
            })
            .collect(),
    }
}

/// The scenario's structure BSP tags (`scnr+0x210`, stride 0x44, tag ID at
/// `+0x14`; docs/live/live-art.md).
pub fn scenario_bsps<R: Read + Seek>(map: &mut Map<R>) -> crate::Result<Vec<TagId>> {
    let scenario = map
        .scenario()
        .ok_or(crate::Error::Invalid("no scenario"))?
        .clone();
    let location = scenario
        .location
        .ok_or(crate::Error::Invalid("no scenario payload"))?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect())
}

/// One drawn part of a BSP section, placed in the world: its own copy of
/// the vertices it uses and its triangles over them.
#[derive(Clone, Debug)]
pub struct WorldPart {
    /// The part's shader (`sbsp +0xa4` material, shader at `+0xc`), or null.
    pub shader: TagId,
    /// Part kind 4, transparent in the tool's part-type names (glass, water,
    /// Ascension's cloud deck). Not drawn opaque, framed or picked.
    pub transparent: bool,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub tris: Vec<[u32; 3]>,
    /// The lightmap page and each vertex's coordinates in it (stream type
    /// `1f`), when the part has both.
    pub lightmap: Option<LightmapRef>,
    pub lightmap_uvs: Vec<[f32; 2]>,
}

impl Default for WorldPart {
    fn default() -> Self {
        Self {
            shader: TagId::NULL,
            transparent: false,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            tris: Vec::new(),
            lightmap: None,
            lightmap_uvs: Vec::new(),
        }
    }
}

/// Part kind of transparent geometry. Only "0 and 5 are not drawn" is
/// engine-backed (docs/formats/structure-bsp.md); 4 is the tool's
/// "transparent", and on the disc only transparent-template shaders use it.
pub const PART_TRANSPARENT: u16 = 4;

/// The scenario's (structure BSP, lightmap) pairs: BSP ID at `+0x14`,
/// lightmap ID at `+0x1c` of each `scnr+0x210` element (as
/// `viewer::level::scenario_bsps`).
pub fn scenario_bsp_lightmaps<R: Read + Seek>(
    map: &mut Map<R>,
) -> crate::Result<Vec<(TagId, Option<TagId>)>> {
    let scenario = map
        .scenario()
        .ok_or(crate::Error::Invalid("no scenario"))?
        .clone();
    let location = scenario
        .location
        .ok_or(crate::Error::Invalid("no scenario payload"))?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| {
            let word = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let lightmap = word(0x1c);
            (
                TagId(word(0x14)),
                (lightmap != u32::MAX).then_some(TagId(lightmap)),
            )
        })
        .collect())
}

/// A lightmap's pages: its bitmap (one P8 image per page) and the palettes
/// the pages' indices go through (`ltmp` group, docs/formats/structure-bsp.md).
#[derive(Clone, Debug)]
pub struct LightmapPages {
    pub bitmap: TagId,
    /// 256 `A8R8G8B8` entries each.
    pub palettes: Vec<[u32; 256]>,
}

/// Which lightmap page lights a part: an index into the list of
/// [`LightmapPages`] [`world_parts_lit`] returns, the image index in its
/// bitmap and the palette index (the cluster's or instance's render info).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LightmapRef {
    pub pages: usize,
    pub page: u16,
    pub palette: u8,
}

impl LightmapRef {
    fn of(pages: Option<usize>, info: Option<&LightmapRenderInfo>) -> Option<Self> {
        let info = info?;
        Some(Self {
            pages: pages?,
            page: u16::try_from(info.bitmap_index).ok()?,
            palette: u8::try_from(info.palette_index).ok()?,
        })
    }
}

/// Every drawn part of the scenario's BSPs (clusters and placed instanced
/// geometry), and how many sections failed.
pub fn world_parts<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
) -> crate::Result<(Vec<WorldPart>, usize)> {
    let (parts, _, failed) = world_parts_lit(map, raw)?;
    Ok((parts, failed))
}

/// [`world_parts`] with each part's lightmap page, and the lightmaps'
/// pages. A lightmap that does not decode leaves its parts unlit.
pub fn world_parts_lit<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
) -> crate::Result<(Vec<WorldPart>, Vec<LightmapPages>, usize)> {
    let mut out = Vec::new();
    let mut lightmaps = Vec::new();
    let mut failed = 0;
    for (id, lightmap) in scenario_bsp_lightmaps(map)? {
        let shaders = bsp_shaders(map, id).unwrap_or_default();
        let bsp = match StructureRender::read(map, id) {
            Ok(bsp) => bsp,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        // The group lighting this BSP: the one with its checksum.
        let group = lightmap
            .and_then(|l| Lightmap::read(map, l).ok())
            .and_then(|l| {
                l.groups
                    .into_iter()
                    .find(|g| g.structure_checksum == bsp.checksum && g.bitmap.is_some())
            });
        let pages = group.as_ref().map(|g| {
            lightmaps.push(LightmapPages {
                bitmap: g.bitmap.unwrap_or(TagId::NULL),
                palettes: g.palettes.clone(),
            });
            lightmaps.len() - 1
        });
        let lit = Lit {
            pages,
            clusters: group.as_ref().map_or(&[], |g| &g.cluster_render_info),
            instances: group.as_ref().map_or(&[], |g| &g.instance_render_info),
        };
        failed += bsp_parts(&bsp, &shaders, &lit, raw, &mut out);
    }
    Ok((out, lightmaps, failed))
}

/// A BSP's lightmap selections.
struct Lit<'a> {
    pages: Option<usize>,
    clusters: &'a [LightmapRenderInfo],
    instances: &'a [LightmapRenderInfo],
}

/// Every opaque triangle of the scenario's BSPs (what the camera frames and
/// a placement drops onto), and how many sections failed. Transparent parts
/// are left out: Ascension's cloud deck is a 700-unit transparent plane
/// below the level that would otherwise hide its lower half and swamp the
/// framing.
pub fn world_triangles<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
) -> crate::Result<(Vec<Tri>, usize)> {
    let (parts, failed) = world_parts(map, raw)?;
    Ok((opaque_triangles(&parts), failed))
}

/// The opaque parts as face-normal triangles.
pub fn opaque_triangles(parts: &[WorldPart]) -> Vec<Tri> {
    let mut out = Vec::new();
    for part in parts.iter().filter(|p| !p.transparent) {
        for t in &part.tris {
            let p = t.map(|i| part.positions[i as usize]);
            let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
            out.push(Tri { p, n });
        }
    }
    out
}

/// Per BSP material, its shader's tag ID (`sbsp +0xa4`, stride 0x20, shader
/// ID at `+0xc`; docs/formats/map-format.md).
pub fn bsp_shaders<R: Read + Seek>(map: &mut Map<R>, bsp: TagId) -> crate::Result<Vec<TagId>> {
    let tag = map.tag(bsp)?.clone();
    let location = tag
        .location
        .ok_or(crate::Error::Invalid("BSP has no payload"))?;
    let bytes = map.read_tag_bytes(bsp)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0xac)?, 0xa4, 0x20)?
        .into_iter()
        .map(|e| TagId(u32::from_le_bytes([e[0xc], e[0xd], e[0xe], e[0xf]])))
        .collect())
}

/// A section's drawn parts, placed by `place` (points) and `turn`
/// (normals). `None` when its vertices do not decode.
pub fn section_parts(
    s: &Section,
    shaders: &[TagId],
    lightmap: Option<LightmapRef>,
    place: &dyn Fn(Vec3) -> Vec3,
    turn: &dyn Fn(Vec3) -> Vec3,
) -> Option<Vec<WorldPart>> {
    let v = s.vertices(None).ok()?;
    if v.positions.len() != s.vertex_count() {
        return None;
    }
    let lightmap = lightmap.filter(|_| v.lightmap_texcoords.len() == v.positions.len());
    Some(
        s.parts
            .iter()
            .filter(|p| p.is_drawn())
            .map(|part| {
                let mut out = WorldPart {
                    shader: shaders
                        .get(usize::from(part.material))
                        .copied()
                        .unwrap_or(TagId::NULL),
                    transparent: part.kind == PART_TRANSPARENT,
                    lightmap,
                    ..WorldPart::default()
                };
                let mut remap = std::collections::HashMap::new();
                for t in s.part_triangles(part) {
                    if t.iter().any(|&i| usize::from(i) >= v.positions.len())
                        || t[0] == t[1]
                        || t[1] == t[2]
                        || t[0] == t[2]
                    {
                        continue;
                    }
                    let tri = t.map(|i| {
                        *remap.entry(i).or_insert_with(|| {
                            let i = usize::from(i);
                            out.positions.push(place(v.positions[i]));
                            out.normals
                                .push(v.normals.get(i).map_or([0.0; 3], |&n| normalize(turn(n))));
                            out.uvs
                                .push(v.texcoords.get(i).copied().unwrap_or([0.0; 2]));
                            if lightmap.is_some() {
                                out.lightmap_uvs.push(v.lightmap_texcoords[i]);
                            }
                            (out.positions.len() - 1) as u32
                        })
                    });
                    out.tris.push(tri);
                }
                if v.normals.is_empty() {
                    out.normals = vertex_normals(&out.positions, &out.tris);
                }
                out
            })
            .filter(|p| !p.tris.is_empty())
            .collect(),
    )
}

/// Smooth normals from the faces around each vertex.
pub fn vertex_normals(positions: &[Vec3], tris: &[[u32; 3]]) -> Vec<Vec3> {
    let mut n = vec![[0.0; 3]; positions.len()];
    for t in tris {
        let p = t.map(|i| positions[i as usize]);
        let f = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        for &i in t {
            n[i as usize] = add(n[i as usize], f);
        }
    }
    n.into_iter().map(normalize).collect()
}

fn bsp_parts(
    bsp: &StructureRender,
    shaders: &[TagId],
    lit: &Lit<'_>,
    raw: &mut dyn RawSource,
    out: &mut Vec<WorldPart>,
) -> usize {
    let materials = Some(bsp.material_count as usize);
    let mut failed = 0;
    let mut push = |s: &Section,
                    lm: Option<LightmapRef>,
                    place: &dyn Fn(Vec3) -> Vec3,
                    turn: &dyn Fn(Vec3) -> Vec3| {
        section_parts(s, shaders, lm, place, turn)
            .map(|p| out.extend(p))
            .is_none()
    };
    for (ci, c) in bsp.clusters.iter().enumerate() {
        let lm = LightmapRef::of(lit.pages, lit.clusters.get(ci));
        match c.geometry.load(raw, materials) {
            Ok(Some(s)) => failed += usize::from(push(&s, lm, &|p| p, &|n| n)),
            Ok(None) => {}
            Err(_) => failed += 1,
        }
    }
    let defs: Vec<Option<Section>> = bsp
        .instance_definitions
        .iter()
        .map(|d| {
            d.geometry.load(raw, materials).unwrap_or_else(|_| {
                failed += 1;
                None
            })
        })
        .collect();
    for (ii, inst) in bsp.instances.iter().enumerate() {
        let lm = LightmapRef::of(lit.pages, lit.instances.get(ii));
        let Some(Some(s)) = usize::try_from(inst.definition)
            .ok()
            .and_then(|i| defs.get(i))
        else {
            continue;
        };
        if !inst.is_rigid() || inst.scale <= 0.0 {
            failed += 1;
            continue;
        }
        failed += usize::from(push(s, lm, &|p| inst.transform(p), &|n| inst.rotate(n)));
    }
    failed
}

/// Rotation from yaw, pitch, roll (radians): `Rz(yaw) · Ry(−pitch) · Rx(roll)`
/// as docs/live/live-art.md established for placements. Columns are the forward,
/// left and up axes.
pub fn rotation(ypr: Vec3) -> [Vec3; 3] {
    let (sy, cy) = ypr[0].sin_cos();
    let (sp, cp) = ypr[1].sin_cos();
    let (sr, cr) = ypr[2].sin_cos();
    let forward = [cy * cp, sy * cp, sp];
    let left = {
        let y = [0.0, cr, sr]; // Rx(r)·ŷ
        let y = [y[0] * cp - y[2] * sp, y[1], y[0] * sp + y[2] * cp]; // Ry(−p)
        [y[0] * cy - y[1] * sy, y[0] * sy + y[1] * cy, y[2]] // Rz(y)
    };
    let up = cross(forward, left);
    [forward, left, up]
}

/// One placement to draw and pick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub block: BlockKind,
    pub index: usize,
    pub position: Vec3,
    /// Forward, left, up.
    pub axes: [Vec3; 3],
    /// Uniform scale (the block's scale field; 1 when it has none or 0).
    pub scale: f32,
    /// The tag it places (palette entry or item reference), if any.
    pub object: Option<TagId>,
}

/// Every element with a position, in block order.
pub fn markers(scenario: &Scenario) -> Vec<Marker> {
    let mut out = Vec::new();
    for b in &scenario.blocks {
        let Some(pos) = b.def.role(Role::Position) else {
            continue;
        };
        let rot = b.def.role(Role::Rotation);
        let facing = b.def.role(Role::Facing);
        for index in 0..b.len() {
            let get = |name| scenario.get(b.def.kind, index, name).ok();
            let Some(Value::Vec3(position)) = get(pos.name) else {
                continue;
            };
            if !position.iter().all(|x| x.is_finite()) {
                continue;
            }
            let ypr = match (rot, facing) {
                (Some(r), _) => get(r.name).and_then(Value::as_vec3).unwrap_or([0.0; 3]),
                (None, Some(f)) => [get(f.name).and_then(Value::as_f32).unwrap_or(0.0), 0.0, 0.0],
                _ => [0.0; 3],
            };
            let ypr = if ypr.iter().all(|x| x.is_finite()) {
                ypr
            } else {
                [0.0; 3]
            };
            let scale = b
                .def
                .role(Role::Scale)
                .and_then(|f| get(f.name))
                .and_then(Value::as_f32)
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(1.0);
            out.push(Marker {
                block: b.def.kind,
                index,
                position,
                axes: rotation(ypr),
                scale,
                object: scenario
                    .object_tag(b.def.kind, index)
                    .map(|r| r.id)
                    .filter(|id| *id != TagId::NULL),
            });
        }
    }
    out
}

/// Marker colour per block (linear RGB).
pub fn block_color(kind: BlockKind) -> Vec3 {
    use BlockKind::*;
    match kind {
        Scenery => [0.55, 0.55, 0.45],
        Vehicles => [0.2, 0.5, 1.0],
        Equipment => [0.2, 0.9, 0.9],
        Weapons => [1.0, 0.6, 0.1],
        Machines => [0.6, 0.4, 0.9],
        SoundScenery => [0.9, 0.9, 0.2],
        Crates => [0.6, 0.35, 0.2],
        StartingLocations => [0.2, 1.0, 0.3],
        NetgameFlags => [1.0, 0.2, 0.6],
        NetgameEquipment => [1.0, 0.25, 0.2],
        StartingEquipment => [0.7, 0.7, 0.7],
    }
}

/// Marker radius in world units (a standing player is under one unit tall).
pub const MARKER_RADIUS: f32 = 0.25;

/// Beyond this distance from the eye, glyphs grow with distance so they
/// keep their size on screen (about 2.5 % of the view's height for a
/// spawn point at 60° field of view): readable when the whole map is
/// framed.
pub const GLYPH_NATIVE_DISTANCE: f32 = 40.0;

/// How much a glyph at `center` is enlarged when seen from `eye`.
pub fn glyph_scale(eye: Vec3, center: Vec3) -> f32 {
    let d = sub(center, eye);
    (dot(d, d).sqrt() / GLYPH_NATIVE_DISTANCE).max(1.0)
}

/// The glyph a placement without a model gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// A diamond stretched along the facing.
    Diamond,
    /// A standing pawn with an arrow at its feet (player spawns).
    Spawn,
    /// A pole with a pennant (netgame flags).
    Flag,
}

pub fn glyph_of(kind: BlockKind) -> Glyph {
    match kind {
        BlockKind::StartingLocations => Glyph::Spawn,
        BlockKind::NetgameFlags => Glyph::Flag,
        _ => Glyph::Diamond,
    }
}

/// Where a glyph is picked: a sphere's centre offset up from the
/// placement, and its radius, at native size.
pub fn glyph_bounds(glyph: Glyph) -> (f32, f32) {
    match glyph {
        Glyph::Diamond => (0.0, MARKER_RADIUS * 1.5),
        Glyph::Spawn => (0.6, 0.7),
        Glyph::Flag => (0.8, 0.8),
    }
}

/// A marker's triangles at native size, by its block's [`Glyph`].
pub fn marker_triangles(m: &Marker) -> Vec<[Vec3; 3]> {
    let [f, l, u] = m.axes;
    let at = |base: Vec3, v: Vec3, s: f32| add(base, scale(v, s));
    let mut out = Vec::new();
    // A double pyramid: tip and back along `axis` from `c`, a ring of four
    // around it across `a` and `b`.
    let mut bipyramid = |c: Vec3, axis: Vec3, ahead: f32, behind: f32, a: Vec3, b: Vec3, r: f32| {
        let tip = at(c, axis, ahead);
        let back = at(c, axis, -behind);
        let ring = [at(c, a, r), at(c, b, r), at(c, a, -r), at(c, b, -r)];
        for i in 0..4 {
            let p = ring[i];
            let q = ring[(i + 1) % 4];
            out.push([tip, q, p]);
            out.push([back, p, q]);
        }
    };
    match glyph_of(m.block) {
        Glyph::Diamond => {
            let r = MARKER_RADIUS;
            bipyramid(m.position, f, 2.0 * r, r, l, u, r);
        }
        Glyph::Spawn => {
            // Body: a tall bipyramid, feet on the placement.
            let up = [0.0, 0.0, 1.0];
            let c = at(m.position, up, 0.6);
            bipyramid(c, up, 0.6, 0.6, f, l, 0.28);
            // An arrow along the facing at the feet.
            let foot = at(m.position, up, 0.04);
            bipyramid(at(foot, f, 0.35), f, 0.6, 0.1, l, up, 0.18);
        }
        Glyph::Flag => {
            let up = [0.0, 0.0, 1.0];
            // Pole.
            let c = at(m.position, up, 0.8);
            bipyramid(c, up, 0.85, 0.85, f, l, 0.06);
            // Pennant along the facing at the top, both faces.
            let top = at(m.position, up, 1.6);
            let low = at(m.position, up, 1.05);
            let tip = at(at(m.position, up, 1.32), f, 0.75);
            out.push([top, low, tip]);
            out.push([top, tip, low]);
        }
    }
    out
}

/// A ray from the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}
impl Ray {
    /// Distance along the ray to a sphere, if hit.
    pub fn sphere(&self, center: Vec3, radius: f32) -> Option<f32> {
        let oc = sub(self.origin, center);
        let b = dot(oc, self.dir);
        let c = dot(oc, oc) - radius * radius;
        let disc = b * b - c;
        if disc < 0.0 {
            return None;
        }
        let t = -b - disc.sqrt();
        let t = if t >= 0.0 { t } else { -b + disc.sqrt() };
        (t >= 0.0).then_some(t)
    }
    /// Where the ray meets the plane through `point` with `normal`.
    pub fn plane(&self, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let d = dot(self.dir, normal);
        if d.abs() < 1e-6 {
            return None;
        }
        let t = dot(sub(point, self.origin), normal) / d;
        (t >= 0.0).then(|| add(self.origin, scale(self.dir, t)))
    }
    /// Möller-Trumbore distance to a triangle.
    pub fn triangle(&self, t: &[Vec3; 3]) -> Option<f32> {
        let e1 = sub(t[1], t[0]);
        let e2 = sub(t[2], t[0]);
        let p = cross(self.dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-9 {
            return None;
        }
        let inv = 1.0 / det;
        let s = sub(self.origin, t[0]);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = cross(s, e1);
        let v = dot(self.dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let d = dot(e2, q) * inv;
        (d > 0.0).then_some(d)
    }
}

/// The nearest marker glyph the ray hits, with its distance along the ray.
/// Glyphs are enlarged with distance from the ray's origin as they are
/// drawn ([`glyph_scale`]).
pub fn pick_glyph(markers: &[Marker], ray: &Ray) -> Option<(f32, BlockKind, usize)> {
    markers
        .iter()
        .filter_map(|m| {
            let (lift, r) = glyph_bounds(glyph_of(m.block));
            let s = glyph_scale(ray.origin, m.position);
            let c = add(m.position, [0.0, 0.0, lift * s]);
            ray.sphere(c, r * s).map(|t| (t, m))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(t, m)| (t, m.block, m.index))
}

/// The nearest marker the ray hits.
pub fn pick(markers: &[Marker], ray: &Ray) -> Option<(BlockKind, usize)> {
    pick_glyph(markers, ray).map(|(_, b, i)| (b, i))
}

/// The nearest BSP surface the ray hits, for dropping a placement.
pub fn surface(tris: &[Tri], ray: &Ray) -> Option<Vec3> {
    tris.iter()
        .filter_map(|t| ray.triangle(&t.p))
        .min_by(f32::total_cmp)
        .map(|t| add(ray.origin, scale(ray.dir, t)))
}

/// How far frame-all stands back, in bounding radii: at 60° the bounds'
/// sphere just fills the view's height at 2; the steep pitch and the
/// level's flatness let it come closer.
pub const FRAME_DISTANCE: f32 = 1.4;

/// The sky's near plane, in its model's units ([`Camera::sky_view_proj`]).
pub const SKY_NEAR: f32 = 0.05;

/// An orbit camera with a fly mode: it looks at `target` from `distance`
/// along yaw and pitch; flying moves the target with the eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            target: [0.0; 3],
            yaw: 0.8,
            pitch: 0.6,
            distance: 30.0,
            fov_y: 60f32.to_radians(),
        }
    }
}
impl Camera {
    /// Frame a bounding box, looking down at it more steeply than the
    /// default so walls in front do not hide the middle.
    pub fn frame(bounds: &Bounds) -> Self {
        let mut c = Self {
            pitch: 0.85,
            ..Self::default()
        };
        if !bounds.is_empty() {
            c.target = bounds.center();
            c.distance = (bounds.radius() * FRAME_DISTANCE).max(5.0);
        }
        c
    }
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // Looking down at the target from above when pitch > 0.
        [-cy * cp, -sy * cp, -sp]
    }
    pub fn eye(&self) -> Vec3 {
        sub(self.target, scale(self.forward(), self.distance))
    }
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw += dx;
        self.pitch = (self.pitch + dy).clamp(-1.5, 1.5);
    }
    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(0.5, 5000.0);
    }
    /// Move the target in camera space: right, up (world z), forward.
    pub fn fly(&mut self, right: f32, up: f32, forward: f32) {
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        self.target = add(
            self.target,
            add(scale(r, right), add(scale(f, forward), [0.0, 0.0, up])),
        );
    }
    /// Pan in the view plane, in pixels scaled to world units at the target.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_h: f32) {
        let units = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        self.target = add(
            self.target,
            add(scale(r, -dx * units), scale(u, dy * units)),
        );
    }
    fn near_far(&self) -> (f32, f32) {
        (
            (self.distance * 0.01).max(0.05),
            self.distance * 4.0 + 2000.0,
        )
    }
    /// Column-major view matrix (right-handed, looking down -z).
    pub fn view(&self) -> Mat4 {
        let eye = self.eye();
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        [
            [r[0], u[0], -f[0], 0.0],
            [r[1], u[1], -f[1], 0.0],
            [r[2], u[2], -f[2], 0.0],
            [-dot(r, eye), -dot(u, eye), dot(f, eye), 1.0],
        ]
    }
    /// Column-major perspective projection for wgpu clip space (depth 0..1).
    pub fn proj(&self, aspect: f32) -> Mat4 {
        let (n, fa) = self.near_far();
        let t = 1.0 / (self.fov_y * 0.5).tan();
        [
            [t / aspect.max(1e-3), 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, fa / (n - fa), -1.0],
            [0.0, 0.0, n * fa / (n - fa), 0.0],
        ]
    }
    /// Column-major view-projection for wgpu clip space (depth 0..1).
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        mul(&self.proj(aspect), &self.view())
    }
    /// The sky's view-projection: the view turned but not moved (the eye
    /// at the origin, so the sky turns with the camera and never comes
    /// nearer), and a projection with no far plane, so a sky model of any
    /// size is drawn whole. Depth runs from 0 at [`SKY_NEAR`] to 1 at
    /// infinity.
    pub fn sky_view_proj(&self, aspect: f32) -> Mat4 {
        let mut view = self.view();
        view[3] = [0.0, 0.0, 0.0, 1.0];
        let t = 1.0 / (self.fov_y * 0.5).tan();
        let proj = [
            [t / aspect.max(1e-3), 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -SKY_NEAR, 0.0],
        ];
        mul(&proj, &view)
    }
    /// The ray through a viewport point given in 0..1 (x right, y down).
    pub fn ray(&self, u: f32, v: f32, aspect: f32) -> Ray {
        let f = self.forward();
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let up = cross(r, f);
        let t = (self.fov_y * 0.5).tan();
        let x = (2.0 * u - 1.0) * t * aspect;
        let y = (1.0 - 2.0 * v) * t;
        Ray {
            origin: self.eye(),
            dir: normalize(add(f, add(scale(r, x), scale(up, y)))),
        }
    }
    /// Project a world point to 0..1 viewport coordinates, if in front.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<[f32; 2]> {
        let m = self.view_proj(aspect);
        let c = transform(&m, [p[0], p[1], p[2], 1.0]);
        (c[3] > 0.0).then(|| [(c[0] / c[3] + 1.0) * 0.5, (1.0 - c[1] / c[3]) * 0.5])
    }
}

/// A column-major 4x4 as the flat array ImGuizmo takes.
pub fn flat(m: &Mat4) -> [f32; 16] {
    let mut out = [0.0; 16];
    for (c, col) in m.iter().enumerate() {
        out[c * 4..c * 4 + 4].copy_from_slice(col);
    }
    out
}

/// Position and yaw/pitch/roll from a column-major model matrix whose
/// columns are the forward, left and up axes (as [`rotation`] builds them),
/// possibly scaled: the inverse of `rotation`. Pitch is clamped to ±90°.
pub fn decompose(m: &[f32; 16]) -> (Vec3, Vec3) {
    let f = normalize([m[0], m[1], m[2]]);
    let l = normalize([m[4], m[5], m[6]]);
    let position = [m[12], m[13], m[14]];
    let pitch = f[2].clamp(-1.0, 1.0).asin();
    let yaw = f[1].atan2(f[0]);
    // Undo Rz(yaw) then Ry(-pitch) on the left axis; what is left is
    // Rx(roll)·ŷ = [0, cos roll, sin roll].
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let a = l[0] * cy + l[1] * sy;
    let ly = -l[0] * sy + l[1] * cy;
    let lz = -a * sp + l[2] * cp;
    let roll = lz.atan2(ly);
    (position, [yaw, pitch, roll])
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}
pub fn transform(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|k| m[k][r] * v[k]).sum();
    }
    out
}
