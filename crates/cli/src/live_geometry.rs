//! Level geometry (multiplayer and campaign) for the stats site:
//! `map/<stem>/map.glb`, the pre-drawn views in `map/<stem>/views/` and
//! `views.json` (docs/live-art.md, the map geometry contract).
//!
//! Runs inside the art extraction (`live_art::extract`), from the owner's own
//! disc, once per disc. The level is the structure BSP's render geometry as
//! the engine draws it: every cluster and every placed instance, through the
//! shared section decoder in `tags::structure` (docs/structure-bsp.md). No
//! layout is re-derived here except the scenario's BSP list (as the `bsp`
//! viewer reads it) and the BSP material element (0x20 bytes, shader
//! reference at +8, the render-model material layout; checked against the
//! disc by the ignored test, and ignored on a map where it does not hold).
//!
//! Nothing from the game is named in the code. A surface's class comes from
//! the facing of the triangle (up-facing is floor) and from generic English
//! words in its shader's tag name read at run time (`glass`, `water`, ...).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use assets::ResourceFiles;
use map::{CacheType, FourCC, Map, RawPointer, TagId};
use serde::{Deserialize, Serialize};
use tags::geometry::{RawSource, Section, TagView};
use tags::structure::StructureRender;

use crate::live_art::{Disc, MapFile};
use crate::live_render::{self, Camera, ShadowMap, VIEWS};

/// The geometry files' format (contract `GEOMETRY_VERSION`).
pub const GEOMETRY_VERSION: u32 = 1;
/// `asset.generator` and `asset.extras.generator` of every `map.glb`.
pub const GENERATOR: &str = "h2 live art";

const SHAD: FourCC = FourCC(*b"shad");

/// A surface class: the glTF material and the colour it is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    Floor,
    Wall,
    Metal,
    Glass,
    Water,
    Foliage,
    Default,
}

impl Class {
    pub const ALL: [Class; 7] = [
        Class::Floor,
        Class::Wall,
        Class::Metal,
        Class::Glass,
        Class::Water,
        Class::Foliage,
        Class::Default,
    ];

    /// The material name (contract).
    pub fn name(self) -> &'static str {
        match self {
            Class::Floor => "floor",
            Class::Wall => "wall",
            Class::Metal => "metal",
            Class::Glass => "glass",
            Class::Water => "water",
            Class::Foliage => "foliage",
            Class::Default => "default",
        }
    }

    /// Base colour (sRGB). Our own calm palette: light warm grey where you
    /// walk, cool slate walls, muted accents.
    pub fn srgb(self) -> [u8; 3] {
        match self {
            Class::Floor => [0x9e, 0xa1, 0x97],
            Class::Wall => [0x7c, 0x86, 0x93],
            Class::Metal => [0x6b, 0x7f, 0x96],
            Class::Glass => [0xa9, 0xd2, 0xe3],
            Class::Water => [0x47, 0x80, 0x8f],
            Class::Foliage => [0x72, 0x8c, 0x5c],
            Class::Default => [0x8e, 0x92, 0x99],
        }
    }

    /// Opacity; below 1 the material blends.
    pub fn alpha(self) -> f32 {
        match self {
            Class::Glass => 0.35,
            Class::Water => 0.75,
            _ => 1.0,
        }
    }

    pub fn transparent(self) -> bool {
        self.alpha() < 1.0
    }

    /// Base colour in linear RGB.
    pub fn linear(self) -> [f32; 3] {
        self.srgb().map(|c| srgb_to_linear(f32::from(c) / 255.0))
    }

    fn metallic_roughness(self) -> (f32, f32) {
        match self {
            Class::Metal => (0.6, 0.45),
            Class::Glass => (0.0, 0.1),
            Class::Water => (0.0, 0.2),
            _ => (0.0, 0.9),
        }
    }
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// What a shader's name says about the surfaces drawn with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// A class the name is specific about.
    Class(Class),
    /// Sky, clouds or fog: not part of the level.
    Sky,
    /// Nothing; the triangle's facing decides.
    Unknown,
}

/// The surface a shader's tag name describes, by generic words in its last
/// path component only.
pub fn surface_from_name(name: &str) -> Surface {
    // Only the last path component: folders are named after the level.
    let own = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let lower = own.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    // Whole words, or for `prefixes` the start of a word ("rusted").
    // Folder names and level names (which may contain "water") never count.
    let has = |exact: &[&str], prefixes: &[&str]| {
        words
            .iter()
            .any(|w| exact.contains(w) || prefixes.iter().any(|p| w.starts_with(p)))
    };
    if has(
        &[
            "sky", "skybox", "skies", "cloud", "clouds", "fog", "mist", "haze",
        ],
        &[],
    ) {
        Surface::Sky
    } else if has(&["glass"], &[]) {
        Surface::Class(Class::Glass)
    } else if has(
        &[
            "water",
            "waterfall",
            "ocean",
            "sea",
            "river",
            "lake",
            "pool",
            "pond",
        ],
        &[],
    ) {
        Surface::Class(Class::Water)
    } else if has(
        &[
            "grass", "plant", "plants", "tree", "trees", "leaf", "leaves", "bush", "bushes",
            "vine", "vines", "fern", "ferns", "hedge", "ivy", "shrub", "shrubs", "palm", "palms",
        ],
        &["foliage"],
    ) {
        Surface::Class(Class::Foliage)
    } else if has(
        &["iron", "pipe", "pipes", "chrome"],
        &["metal", "steel", "grate", "grating", "rust", "alumin"],
    ) {
        Surface::Class(Class::Metal)
    } else {
        Surface::Unknown
    }
}

/// The class a triangle is drawn as: water, foliage and glass by name; then
/// anything facing up is floor; metal by name; other steep faces are walls;
/// overhangs and ceilings are `default`.
pub fn classify(surface: Surface, face_normal: [f32; 3]) -> Class {
    match surface {
        Surface::Class(c @ (Class::Water | Class::Foliage | Class::Glass)) => return c,
        _ if face_normal[2] >= 0.6 => return Class::Floor,
        Surface::Class(c) => return c,
        _ => {}
    }
    if face_normal[2] > -0.6 {
        Class::Wall
    } else {
        Class::Default
    }
}

/// One world-space triangle (game coordinates, z up) with vertex normals.
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub p: [[f32; 3]; 3],
    pub n: [[f32; 3]; 3],
    pub surface: Surface,
}

/// Every drawn triangle of a BSP: clusters as stored, instances placed. A
/// section that fails to load is counted in the second value, not fatal.
pub fn bsp_triangles(
    bsp: &StructureRender,
    raw: &mut dyn RawSource,
    surfaces: &[Surface],
) -> (Vec<Tri>, usize) {
    let materials = Some(bsp.material_count as usize);
    let mut out = Vec::new();
    let mut failed = 0;
    let push = |s: &Section,
                place: &dyn Fn([f32; 3]) -> [f32; 3],
                rotate: &dyn Fn([f32; 3]) -> [f32; 3],
                out: &mut Vec<Tri>|
     -> Result<()> {
        let v = s.vertices(None)?;
        if v.positions.len() != s.vertex_count() {
            return Err(anyhow!("section without float positions"));
        }
        let placed: Vec<[f32; 3]> = v.positions.iter().map(|&p| place(p)).collect();
        let normals: Vec<[f32; 3]> = if v.normals.len() == placed.len() {
            v.normals.iter().map(|&n| rotate(n)).collect()
        } else {
            vec![[0.0; 3]; placed.len()]
        };
        for part in s.parts.iter().filter(|p| p.is_drawn()) {
            let surface = surfaces
                .get(usize::from(part.material))
                .copied()
                .unwrap_or(Surface::Unknown);
            if surface == Surface::Sky {
                continue;
            }
            for t in s.part_triangles(part) {
                let i = t.map(usize::from);
                out.push(Tri {
                    p: i.map(|i| placed[i]),
                    n: i.map(|i| normals[i]),
                    surface,
                });
            }
        }
        Ok(())
    };
    let same = |p: [f32; 3]| p;
    for c in &bsp.clusters {
        match c.geometry.load(raw, materials) {
            Ok(Some(s)) => {
                if push(&s, &same, &same, &mut out).is_err() {
                    failed += 1;
                }
            }
            Ok(None) => {}
            Err(_) => failed += 1,
        }
    }
    let defs: Vec<Option<Section>> = bsp
        .instance_definitions
        .iter()
        .map(|d| match d.geometry.load(raw, materials) {
            Ok(s) => s,
            Err(_) => {
                failed += 1;
                None
            }
        })
        .collect();
    for inst in &bsp.instances {
        let Some(Some(s)) = defs.get(inst.definition as usize) else {
            continue;
        };
        if !inst.is_rigid() || inst.scale <= 0.0 {
            failed += 1;
            continue;
        }
        if push(s, &|p| inst.transform(p), &|n| inst.rotate(n), &mut out).is_err() {
            failed += 1;
        }
    }
    (out, failed)
}

/// Axis-aligned bounds in game coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn add(&mut self, p: [f32; 3]) {
        for (k, v) in p.into_iter().enumerate() {
            self.min[k] = self.min[k].min(v);
            self.max[k] = self.max[k].max(v);
        }
    }

    pub fn corners(&self) -> [[f32; 3]; 8] {
        std::array::from_fn(|i| {
            [
                if i & 1 == 0 { self.min[0] } else { self.max[0] },
                if i & 2 == 0 { self.min[1] } else { self.max[1] },
                if i & 4 == 0 { self.min[2] } else { self.max[2] },
            ]
        })
    }

    /// Grown by `f` of its size on every side.
    pub fn grown(&self, f: f32) -> Bounds {
        let mut b = *self;
        for k in 0..3 {
            let pad = (self.max[k] - self.min[k]) * f;
            b.min[k] -= pad;
            b.max[k] += pad;
        }
        b
    }
}

/// One glTF primitive: the welded triangles of one class (game coordinates).
#[derive(Clone, Debug, Default)]
pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// The level, welded, one primitive per class present.
#[derive(Clone, Debug)]
pub struct LevelMesh {
    pub primitives: Vec<(Class, Primitive)>,
    pub bounds: Bounds,
}

impl LevelMesh {
    pub fn triangles(&self) -> usize {
        self.primitives
            .iter()
            .map(|(_, p)| p.indices.len() / 3)
            .sum()
    }
    pub fn vertices(&self) -> usize {
        self.primitives.iter().map(|(_, p)| p.positions.len()).sum()
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 0.0 { a.map(|v| v / l) } else { a }
}

/// Smallest triangle kept: twice its area, in square world units.
const MIN_DOUBLE_AREA: f32 = 1e-6;
/// Welding grid: 1/1024 world unit; normals to 1/64.
const WELD: f32 = 1024.0;

/// Classify, drop degenerate and duplicate triangles, and weld vertices that
/// share a position and normal within a class. A vertex normal that is
/// missing or points against its triangle is replaced by the face normal.
pub fn build_mesh(tris: &[Tri]) -> LevelMesh {
    struct Builder {
        prim: Primitive,
        index: HashMap<([i32; 3], [i8; 3]), u32>,
        seen: HashSet<[u32; 3]>,
    }
    let mut builders: BTreeMap<Class, Builder> = BTreeMap::new();
    let mut bounds = Bounds::EMPTY;
    for t in tris {
        if !t.p.iter().flatten().all(|v| v.is_finite()) {
            continue;
        }
        let face = cross(sub(t.p[1], t.p[0]), sub(t.p[2], t.p[0]));
        let len = dot(face, face).sqrt();
        if len.is_nan() || len < MIN_DOUBLE_AREA {
            continue;
        }
        let face = face.map(|v| v / len);
        let class = classify(t.surface, face);
        let b = builders.entry(class).or_insert_with(|| Builder {
            prim: Primitive::default(),
            index: HashMap::new(),
            seen: HashSet::new(),
        });
        let mut idx = [0u32; 3];
        for (k, slot) in idx.iter_mut().enumerate() {
            let n = t.n[k];
            let l = dot(n, n).sqrt();
            let n = if l > 0.5 && n.iter().all(|v| v.is_finite()) && dot(n, face) > 0.0 {
                n.map(|v| v / l)
            } else {
                face
            };
            let p = t.p[k];
            let key = (
                p.map(|v| (v * WELD).round() as i32),
                n.map(|v| (v * 64.0).round() as i8),
            );
            *slot = *b.index.entry(key).or_insert_with(|| {
                b.prim.positions.push(p);
                b.prim.normals.push(n);
                (b.prim.positions.len() - 1) as u32
            });
        }
        if idx[0] == idx[1] || idx[1] == idx[2] || idx[0] == idx[2] {
            continue;
        }
        // The same triangle twice (same winding) is drawn once.
        let r = idx.iter().enumerate().min_by_key(|(_, v)| **v).unwrap().0;
        let canon = [idx[r], idx[(r + 1) % 3], idx[(r + 2) % 3]];
        if !b.seen.insert(canon) {
            continue;
        }
        b.prim.indices.extend(idx);
        for p in t.p {
            bounds.add(p);
        }
    }
    LevelMesh {
        primitives: builders
            .into_iter()
            .filter(|(_, b)| !b.prim.indices.is_empty())
            .map(|(c, b)| (c, b.prim))
            .collect(),
        bounds,
    }
}

/// Grid cells per side for [`frame`].
const FRAME_GRID: usize = 64;

/// The region the views frame and clip to: where the level is detailed, not
/// its far backdrop. Triangle centroids are counted on a grid over the
/// level's x/y bounds; cells with at least a tenth of the 90th-percentile
/// count are dense. Each 8-connected group of dense cells is a candidate;
/// every group with at least 15% of the largest group's triangles is kept,
/// grown by a cell. z spans the kept centroids, less a few outliers cut off
/// by a wide gap ([`z_range`]). Everything is padded by 3% and kept inside
/// the bounds.
pub fn frame(mesh: &LevelMesh) -> Bounds {
    let b = mesh.bounds;
    let centroids: Vec<[f32; 3]> = mesh
        .primitives
        .iter()
        .flat_map(|(_, p)| {
            p.indices.as_chunks::<3>().0.iter().map(|t| {
                let [a, b, c] = t.map(|i| p.positions[i as usize]);
                std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0)
            })
        })
        .collect();
    if centroids.len() < 64 {
        return b;
    }
    let g = FRAME_GRID;
    let cell = ((b.max[0] - b.min[0]).max(b.max[1] - b.min[1]) / g as f32).max(1e-3);
    let at = |c: &[f32; 3]| {
        let x = (((c[0] - b.min[0]) / cell) as usize).min(g - 1);
        let y = (((c[1] - b.min[1]) / cell) as usize).min(g - 1);
        y * g + x
    };
    let mut count = vec![0usize; g * g];
    for c in &centroids {
        count[at(c)] += 1;
    }
    let mut filled: Vec<usize> = count.iter().copied().filter(|&n| n > 0).collect();
    filled.sort_unstable();
    let p90 = filled[filled.len() * 9 / 10];
    let dense = |i: usize| count[i] >= (p90 / 10).max(3);
    // 8-connected groups of dense cells, with their triangle counts.
    let mut group = vec![usize::MAX; g * g];
    let mut sizes = Vec::new();
    for start in 0..g * g {
        if group[start] != usize::MAX || !dense(start) {
            continue;
        }
        let id = sizes.len();
        let mut total = 0;
        let mut stack = vec![start];
        group[start] = id;
        while let Some(i) = stack.pop() {
            total += count[i];
            let (x, y) = ((i % g) as i64, (i / g) as i64);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if !(0..g as i64).contains(&nx) || !(0..g as i64).contains(&ny) {
                        continue;
                    }
                    let j = ny as usize * g + nx as usize;
                    if group[j] == usize::MAX && dense(j) {
                        group[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        sizes.push(total);
    }
    let Some(&largest) = sizes.iter().max() else {
        return b;
    };
    let kept = |i: usize| group[i] != usize::MAX && sizes[group[i]] * 100 >= largest * 15;
    let (mut x0, mut x1, mut y0, mut y1) = (g, 0, g, 0);
    for i in (0..g * g).filter(|&i| kept(i)) {
        let (x, y) = (i % g, i / g);
        x0 = x0.min(x.saturating_sub(1));
        x1 = x1.max((x + 1).min(g - 1));
        y0 = y0.min(y.saturating_sub(1));
        y1 = y1.max((y + 1).min(g - 1));
    }
    let mut z: Vec<f32> = centroids
        .iter()
        .filter(|c| kept(at(c)))
        .map(|c| c[2])
        .collect();
    z.sort_unstable_by(f32::total_cmp);
    let (z0, z1) = z_range(&z);
    let mut out = Bounds {
        min: [b.min[0] + x0 as f32 * cell, b.min[1] + y0 as f32 * cell, z0],
        max: [
            b.min[0] + (x1 + 1) as f32 * cell,
            b.min[1] + (y1 + 1) as f32 * cell,
            z1,
        ],
    }
    .grown(0.03);
    for k in 0..3 {
        out.min[k] = out.min[k].max(b.min[k]);
        out.max[k] = out.max[k].min(b.max[k]);
    }
    out
}

/// The z span of sorted heights: the whole range, except that a sparse tail
/// (under 5% of the values at either end) separated from the rest by a gap
/// of more than 10% of the range is left out, such as a ground plane far
/// below a floating level.
pub fn z_range(z: &[f32]) -> (f32, f32) {
    let n = z.len();
    let (mut lo, mut hi) = (0, n - 1);
    let span = z[n - 1] - z[0];
    let tail = n / 20;
    for i in 0..tail {
        if z[i + 1] - z[i] > 0.1 * span {
            lo = i + 1;
        }
    }
    for i in (n - 1 - tail..n - 1).rev() {
        if z[i + 1] - z[i] > 0.1 * span {
            hi = i;
        }
    }
    (z[lo], z[hi])
}

/// A game position as a glTF point: `(x, y, z)` -> `(x, z, -y)`.
pub fn to_gltf(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

/// Most vertices one primitive may have, so its indices fit in u16.
pub const MAX_PRIMITIVE_VERTICES: usize = 1 << 16;

/// A primitive cut into pieces of at most [`MAX_PRIMITIVE_VERTICES`]
/// vertices each, triangles in order (a small one is returned whole).
pub fn split_u16(p: &Primitive) -> Vec<Primitive> {
    if p.positions.len() <= MAX_PRIMITIVE_VERTICES {
        return vec![p.clone()];
    }
    let mut out = Vec::new();
    let mut cur = Primitive::default();
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for t in p.indices.as_chunks::<3>().0 {
        let new = t.iter().filter(|i| !remap.contains_key(i)).count();
        if cur.positions.len() + new > MAX_PRIMITIVE_VERTICES {
            out.push(std::mem::take(&mut cur));
            remap.clear();
        }
        for &i in t {
            let j = *remap.entry(i).or_insert_with(|| {
                cur.positions.push(p.positions[i as usize]);
                cur.normals.push(p.normals[i as usize]);
                (cur.positions.len() - 1) as u32
            });
            cur.indices.push(j);
        }
    }
    if !cur.indices.is_empty() {
        out.push(cur);
    }
    out
}

/// One node of `map.glb`: an optional mesh (its primitives, game
/// coordinates), `extras`, and child nodes. Nodes carry no transforms.
pub struct GlbNode<'a> {
    pub name: String,
    pub mesh: Option<&'a [(Class, Primitive)]>,
    pub extras: Option<serde_json::Value>,
    pub children: Vec<GlbNode<'a>>,
}

impl<'a> GlbNode<'a> {
    pub fn mesh(name: impl Into<String>, mesh: &'a LevelMesh) -> Self {
        GlbNode {
            name: name.into(),
            mesh: Some(&mesh.primitives),
            extras: None,
            children: Vec::new(),
        }
    }
}

/// `map.glb` of a single mesh: one node and mesh named `structure` (the
/// multiplayer contract), its bounds and [`frame`].
#[cfg(test)]
pub fn write_glb(mesh: &LevelMesh) -> Vec<u8> {
    write_nodes(
        &[GlbNode::mesh("structure", mesh)],
        mesh.bounds,
        frame(mesh),
    )
}

/// `map.glb` from nodes: a mesh per node that has triangles, named like
/// its node, with a primitive per class (POSITION, NORMAL, u16 indices; a
/// class over [`MAX_PRIMITIVE_VERTICES`] vertices gets several) and
/// one material per class shared by every mesh. `asset.extras` carries
/// `bounds` and the views' `frame` (game coordinates).
pub fn write_nodes(roots: &[GlbNode], bounds: Bounds, frame: Bounds) -> Vec<u8> {
    use serde_json::{Value, json};
    struct Out {
        bin: Vec<u8>,
        views: Vec<Value>,
        accessors: Vec<Value>,
        materials: Vec<Value>,
        material_of: BTreeMap<Class, usize>,
        meshes: Vec<Value>,
        nodes: Vec<Value>,
    }
    impl Out {
        fn view(&mut self, bytes: &[u8], target: u32) -> usize {
            while !self.bin.len().is_multiple_of(4) {
                self.bin.push(0);
            }
            self.views.push(json!({
                "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len(), "target": target
            }));
            self.bin.extend_from_slice(bytes);
            self.views.len() - 1
        }
        fn material(&mut self, class: Class) -> usize {
            if let Some(&m) = self.material_of.get(&class) {
                return m;
            }
            let [r, g, b] = class.linear();
            let (metallic, roughness) = class.metallic_roughness();
            let mut m = json!({
                "name": class.name(),
                "pbrMetallicRoughness": {
                    "baseColorFactor": [r, g, b, class.alpha()],
                    "metallicFactor": metallic,
                    "roughnessFactor": roughness
                }
            });
            if class.transparent() {
                m["alphaMode"] = json!("BLEND");
            }
            self.materials.push(m);
            self.material_of.insert(class, self.materials.len() - 1);
            self.materials.len() - 1
        }
        fn primitive(&mut self, class: Class, p: &Primitive) -> Value {
            let pos: Vec<[f32; 3]> = p.positions.iter().map(|&v| to_gltf(v)).collect();
            let nrm: Vec<[f32; 3]> = p.normals.iter().map(|&v| to_gltf(v)).collect();
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for v in &pos {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            let floats = |v: &[[f32; 3]]| -> Vec<u8> {
                v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
            };
            let pv = self.view(&floats(&pos), 34962);
            let nv = self.view(&floats(&nrm), 34962);
            let indices: Vec<u8> = p
                .indices
                .iter()
                .flat_map(|&i| (i as u16).to_le_bytes())
                .collect();
            let iv = self.view(&indices, 34963);
            let base = self.accessors.len();
            self.accessors.push(json!({
                "bufferView": pv, "componentType": 5126, "count": pos.len(), "type": "VEC3",
                "min": lo, "max": hi
            }));
            self.accessors.push(json!({
                "bufferView": nv, "componentType": 5126, "count": nrm.len(), "type": "VEC3"
            }));
            self.accessors.push(json!({
                "bufferView": iv, "componentType": 5123, "count": p.indices.len(), "type": "SCALAR"
            }));
            json!({
                "attributes": {"POSITION": base, "NORMAL": base + 1},
                "indices": base + 2,
                "material": self.material(class),
                "mode": 4
            })
        }
        /// Adds a node and its children; returns its index.
        fn node(&mut self, n: &GlbNode) -> usize {
            let at = self.nodes.len();
            self.nodes.push(json!({"name": n.name}));
            let prims: Vec<Value> = n
                .mesh
                .unwrap_or(&[])
                .iter()
                .filter(|(_, p)| !p.indices.is_empty())
                .flat_map(|(c, p)| split_u16(p).into_iter().map(|p| (*c, p)))
                .map(|(c, p)| self.primitive(c, &p))
                .collect();
            if !prims.is_empty() {
                self.meshes
                    .push(json!({"name": n.name, "primitives": prims}));
                self.nodes[at]["mesh"] = json!(self.meshes.len() - 1);
            }
            if let Some(e) = &n.extras {
                self.nodes[at]["extras"] = e.clone();
            }
            if !n.children.is_empty() {
                let kids: Vec<usize> = n.children.iter().map(|c| self.node(c)).collect();
                self.nodes[at]["children"] = json!(kids);
            }
            at
        }
    }
    let mut o = Out {
        bin: Vec::new(),
        views: Vec::new(),
        accessors: Vec::new(),
        materials: Vec::new(),
        material_of: BTreeMap::new(),
        meshes: Vec::new(),
        nodes: Vec::new(),
    };
    let top: Vec<usize> = roots.iter().map(|n| o.node(n)).collect();
    while !o.bin.len().is_multiple_of(4) {
        o.bin.push(0);
    }
    let mut doc = json!({
        "asset": {
            "version": "2.0",
            "generator": GENERATOR,
            "extras": {
                "generator": GENERATOR,
                "geometry_version": GEOMETRY_VERSION,
                "bounds": {"min": bounds.min, "max": bounds.max},
                "frame": {"min": frame.min, "max": frame.max}
            }
        },
        "scene": 0,
        "scenes": [{"nodes": top}],
        "nodes": o.nodes,
    });
    // A file with no geometry has no meshes and no buffer.
    if !o.meshes.is_empty() {
        doc["meshes"] = json!(o.meshes);
        doc["materials"] = json!(o.materials);
        doc["accessors"] = json!(o.accessors);
        doc["bufferViews"] = json!(o.views);
        doc["buffers"] = json!([{"byteLength": o.bin.len()}]);
    } else {
        o.bin.clear();
    }
    let bin = o.bin;
    let mut json = serde_json::to_vec(&doc).expect("glTF JSON");
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend(0x4654_6c67u32.to_le_bytes()); // "glTF"
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(0x4e4f_534au32.to_le_bytes()); // "JSON"
    out.extend(&json);
    if !bin.is_empty() {
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004e_4942u32.to_le_bytes()); // "BIN\0"
        out.extend(&bin);
    }
    out
}

/// One pre-drawn view in `views.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewEntry {
    pub id: String,
    pub kind: String,
    pub width: u32,
    pub height: u32,
    /// `px = M[0]·(x, y, z, 1)`, `py = M[1]·(x, y, z, 1)`, game coordinates,
    /// pixel origin top-left (pixel `i` spans `[i, i + 1)`).
    pub world_to_pixel: [[f64; 4]; 2],
    /// The image, relative to `views.json`.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_sorted: Option<bool>,
    /// Roofs and ceilings over walkable space are cut away, so the picture
    /// shows the floors players stand on (the top view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutaway: Option<bool>,
    /// Compass direction the camera looks from (degrees from +x towards +y);
    /// absent for the top view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f32>,
    pub elevation_deg: f32,
    /// Pixels per world unit.
    pub scale: f32,
    /// Unit vector the camera looks along (game coordinates).
    pub forward: [f32; 3],
}

/// `views.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewsFile {
    pub geometry_version: u32,
    pub map: String,
    pub bounds: Bounds,
    /// The part of the level the views are fitted to.
    pub frame: Bounds,
    pub views: Vec<ViewEntry>,
}

/// Which kind of level a map is (manifest `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapKind {
    Campaign,
    Multiplayer,
}

/// The manifest's entry for one map (contract: `geometry`, `views`, `kind`
/// and `bsps`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub views: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MapKind>,
    /// Structure BSPs in `map.glb` (one node each).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsps: Option<usize>,
}

/// Counts and timings for one map.
#[derive(Clone, Debug, Default)]
pub struct MapStats {
    pub stem: String,
    pub kind: Option<MapKind>,
    /// BSPs in the scenario, and how many made it into `map.glb`.
    pub scenario_bsps: usize,
    pub bsps: usize,
    pub source_triangles: usize,
    pub triangles: usize,
    pub vertices: usize,
    pub failed_sections: usize,
    pub glb_bytes: usize,
    pub classes: BTreeMap<&'static str, usize>,
    pub shaders_named: usize,
    pub materials: usize,
    pub load: Duration,
    pub render: Duration,
}

/// What [`extract`] wrote.
#[derive(Default)]
pub struct Extracted {
    pub maps: BTreeMap<String, MapEntry>,
    pub skipped: BTreeMap<String, String>,
    pub stats: Vec<MapStats>,
}

/// Raw blocks through the art extractor's resource files.
struct Raw<'a>(&'a mut ResourceFiles);
impl RawSource for Raw<'_> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0.read(p.file, p.offset, size).map_err(|e| match e {
            assets::Error::Map(e) => tags::Error::Map(e),
            assets::Error::Io(e) => tags::Error::Map(e.into()),
            assets::Error::MissingResource(f) => tags::Error::MissingResource(f),
            _ => tags::Error::Invalid("raw block read rejected"),
        })
    }
}

type MapReader = Map<BufReader<File>>;

/// The scenario's BSP tags: block at +0x210, stride 0x44, BSP tag ID at
/// +0x14 (as `viewer/src/bin/bsp.rs` reads it).
fn scenario_bsps(map: &mut MapReader) -> Result<Vec<TagId>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect())
}

/// Per BSP material, its shader's tag name. Element 0x20 bytes, shader tag
/// ID at +0xc (the render-model material layout, docs/map-format.md). If any
/// element's ID is neither null nor a `shad` tag, the layout does not hold
/// here and the answer is `None`.
fn material_shaders(map: &mut MapReader, bsp: TagId) -> Result<Option<Vec<Option<String>>>> {
    let tag = map.tag(bsp)?.clone();
    let location = tag.location.context("BSP has no payload")?;
    let bytes = map.read_tag_bytes(bsp)?;
    let view = TagView::new(&bytes, location.address);
    let mut out = Vec::new();
    for e in view.block(view.at(0, 0xac)?, 0xa4, 0x20)? {
        let id = u32::from_le_bytes([e[0xc], e[0xd], e[0xe], e[0xf]]);
        if id == u32::MAX {
            out.push(None);
            continue;
        }
        match map.tag(TagId(id)) {
            Ok(t) if t.group == SHAD => out.push(Some(t.name.clone().unwrap_or_default())),
            _ => return Ok(None),
        }
    }
    Ok(Some(out))
}

/// Per BSP material, what its shader's name says, and how many shaders
/// were found.
fn material_surfaces(map: &mut MapReader, bsp: TagId) -> (Vec<Surface>, usize) {
    match material_shaders(map, bsp) {
        Ok(Some(names)) => (
            names
                .iter()
                .map(|n| n.as_deref().map_or(Surface::Unknown, surface_from_name))
                .collect(),
            names.iter().flatten().count(),
        ),
        _ => (Vec::new(), 0),
    }
}

/// A loaded map, ready to draw.
pub struct Level {
    pub stem: String,
    pub kind: MapKind,
    /// How many structure BSPs the scenario lists.
    pub bsp_count: usize,
    /// Every BSP that loaded: its index in the scenario and its mesh.
    pub bsps: Vec<(usize, LevelMesh)>,
    /// All of them together, for the views.
    pub mesh: LevelMesh,
    pub frame: Bounds,
    pub shadow: ShadowMap,
    /// What was left out of this level, by manifest `skipped` key.
    pub skipped: BTreeMap<String, String>,
}

impl Level {
    /// The level from its BSP meshes (scenario index, mesh or why it failed).
    /// Fails only when no BSP has drawn geometry.
    pub fn build(
        stem: &str,
        kind: MapKind,
        bsps: Vec<(usize, Result<LevelMesh>)>,
    ) -> Result<Level> {
        let bsp_count = bsps.len();
        let mut skipped = BTreeMap::new();
        let mut kept = Vec::new();
        for (i, m) in bsps {
            match m {
                Ok(m) if !m.primitives.is_empty() => kept.push((i, m)),
                Ok(_) => {
                    skipped.insert(
                        format!("map/{stem}/bsp_{i}"),
                        "no drawn level geometry".into(),
                    );
                }
                Err(e) => {
                    skipped.insert(format!("map/{stem}/bsp_{i}"), format!("{e:#}"));
                }
            }
        }
        if kept.is_empty() {
            return Err(match skipped.into_values().next() {
                Some(why) if bsp_count == 1 => anyhow!(why),
                _ => anyhow!("no drawn level geometry"),
            });
        }
        let mesh = merge(kept.iter().map(|(_, m)| m));
        let frame = frame(&mesh);
        let shadow = live_render::shadow_map(&mesh, &frame);
        Ok(Level {
            stem: stem.to_string(),
            kind,
            bsp_count,
            bsps: kept,
            mesh,
            frame,
            shadow,
            skipped,
        })
    }

    /// The `map.glb` nodes: `structure` when the scenario has one BSP,
    /// else `bsp_<index>` per BSP (scenario index) with `extras.bsp`.
    pub fn nodes(&self) -> Vec<GlbNode<'_>> {
        self.bsps
            .iter()
            .map(|(i, m)| {
                if self.bsp_count == 1 {
                    GlbNode::mesh("structure", m)
                } else {
                    GlbNode {
                        extras: Some(serde_json::json!({ "bsp": i })),
                        ..GlbNode::mesh(format!("bsp_{i}"), m)
                    }
                }
            })
            .collect()
    }

    pub fn glb(&self) -> Vec<u8> {
        write_nodes(&self.nodes(), self.mesh.bounds, self.frame)
    }
}

/// Meshes concatenated, a primitive per class.
pub fn merge<'a>(meshes: impl IntoIterator<Item = &'a LevelMesh>) -> LevelMesh {
    let mut by: BTreeMap<Class, Primitive> = BTreeMap::new();
    let mut bounds = Bounds::EMPTY;
    for m in meshes {
        for k in [m.bounds.min, m.bounds.max] {
            bounds.add(k);
        }
        for (c, p) in &m.primitives {
            let out = by.entry(*c).or_default();
            let base = out.positions.len() as u32;
            out.positions.extend(&p.positions);
            out.normals.extend(&p.normals);
            out.indices.extend(p.indices.iter().map(|i| i + base));
        }
    }
    LevelMesh {
        primitives: by.into_iter().collect(),
        bounds,
    }
}

/// Opens one map and builds its level, one mesh per scenario BSP. A BSP
/// that cannot be read is left out (and listed in `Level::skipped`).
fn load_level(disc: &Disc, m: &MapFile, kind: MapKind, stats: &mut MapStats) -> Result<Level> {
    let (mut map, mut files) = disc.open(m)?;
    let mut bsps = Vec::new();
    for (i, bsp_id) in scenario_bsps(&mut map)?.into_iter().enumerate() {
        let mesh = (|| {
            if bsp_id == TagId::NULL {
                return Err(anyhow!("no BSP tag"));
            }
            let (surfaces, named) = material_surfaces(&mut map, bsp_id);
            stats.shaders_named += named;
            stats.materials += surfaces.len();
            let bsp = StructureRender::read(&mut map, bsp_id)
                .with_context(|| format!("reading BSP {bsp_id}"))?;
            let (t, failed) = bsp_triangles(&bsp, &mut Raw(&mut files), &surfaces);
            stats.failed_sections += failed;
            stats.source_triangles += t.len();
            Ok(build_mesh(&t))
        })();
        bsps.push((i, mesh));
    }
    let level = Level::build(m.name(), kind, bsps)?;
    stats.kind = Some(kind);
    stats.scenario_bsps = level.bsp_count;
    stats.bsps = level.bsps.len();
    stats.triangles = level.mesh.triangles();
    stats.vertices = level.mesh.vertices();
    for (c, p) in &level.mesh.primitives {
        stats.classes.insert(c.name(), p.indices.len() / 3);
    }
    Ok(level)
}

/// The view entries and cameras for a level.
pub fn cameras(mesh: &LevelMesh, level_frame: &Bounds) -> Vec<(ViewEntry, Camera)> {
    VIEWS
        .iter()
        .map(|spec| {
            let cam = live_render::fit(spec, mesh, level_frame);
            let entry = ViewEntry {
                id: spec.id.to_string(),
                kind: "ortho".into(),
                width: spec.width,
                height: spec.height,
                world_to_pixel: cam.world_to_pixel(),
                file: format!("views/{}.png", spec.id),
                up: spec.azimuth.is_none().then(|| "north".to_string()),
                depth_sorted: spec.azimuth.is_some().then_some(true),
                cutaway: spec.azimuth.is_none().then_some(true),
                azimuth_deg: spec.azimuth,
                elevation_deg: spec.azimuth.map_or(90.0, |_| live_render::ELEVATION_DEG),
                scale: cam.scale,
                forward: cam.forward,
            };
            (entry, cam)
        })
        .collect()
}

/// Builds, draws and writes every multiplayer map's geometry under `out`
/// (`map/<stem>/...`). Maps load in parallel, then every view of every map
/// is drawn in parallel.
pub fn extract(disc: &Disc, out: &Path) -> Extracted {
    let maps: Vec<(&MapFile, MapKind)> = disc
        .maps_of(CacheType::Multiplayer)
        .map(|m| (m, MapKind::Multiplayer))
        .chain(
            disc.maps_of(CacheType::Campaign)
                .map(|m| (m, MapKind::Campaign)),
        )
        .collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut result = Extracted::default();

    // Load and write map.glb, one map per worker at a time.
    let loaded: Mutex<Vec<(Level, MapStats)>> = Mutex::new(Vec::new());
    let skipped: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..workers.min(maps.len()) {
            s.spawn(|| {
                while let Some(&(m, kind)) = maps.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let started = Instant::now();
                    let mut stats = MapStats {
                        stem: m.name().to_string(),
                        ..Default::default()
                    };
                    let key = format!("map/{}/geometry", m.name());
                    let done = load_level(disc, m, kind, &mut stats).and_then(|level| {
                        let glb = level.glb();
                        stats.glb_bytes = glb.len();
                        let dir = out.join("map").join(&level.stem);
                        fs::create_dir_all(dir.join("views"))?;
                        fs::write(dir.join("map.glb"), glb)?;
                        Ok(level)
                    });
                    stats.load = started.elapsed();
                    match done {
                        Ok(level) => loaded.lock().unwrap().push((level, stats)),
                        Err(e) => {
                            skipped.lock().unwrap().insert(key, format!("{e:#}"));
                        }
                    }
                }
            });
        }
    });
    let mut loaded = loaded.into_inner().unwrap();
    loaded.sort_by(|a, b| a.0.stem.cmp(&b.0.stem));
    result.skipped = skipped.into_inner().unwrap();

    // Draw every (map, view).
    let plans: Vec<Vec<(ViewEntry, Camera)>> = loaded
        .iter()
        .map(|(l, _)| cameras(&l.mesh, &l.frame))
        .collect();
    let jobs: Vec<(usize, usize)> = plans
        .iter()
        .enumerate()
        .flat_map(|(i, p)| (0..p.len()).map(move |v| (i, v)))
        .collect();
    let times: Mutex<Vec<Duration>> = Mutex::new(vec![Duration::ZERO; loaded.len()]);
    let failed: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..workers.min(jobs.len()) {
            s.spawn(|| {
                while let Some(&(i, v)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let started = Instant::now();
                    let level = &loaded[i].0;
                    let (entry, cam) = &plans[i][v];
                    let rgba = live_render::render(
                        &level.mesh,
                        &level.frame,
                        &level.shadow,
                        cam,
                        entry.width as usize,
                        entry.height as usize,
                        entry.cutaway == Some(true),
                    );
                    let path = out.join("map").join(&level.stem).join(&entry.file);
                    let mut png = Vec::new();
                    let written =
                        assets::png::write_rgba_fast(&mut png, entry.width, entry.height, &rgba)
                            .map_err(anyhow::Error::from)
                            .and_then(|_| Ok(fs::write(&path, png)?));
                    if let Err(e) = written {
                        failed.lock().unwrap().insert(
                            format!("map/{}/views/{}", level.stem, entry.id),
                            format!("{e:#}"),
                        );
                    }
                    times.lock().unwrap()[i] += started.elapsed();
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    let times = times.into_inner().unwrap();
    for (i, ((level, mut stats), plan)) in loaded.into_iter().zip(plans).enumerate() {
        stats.render = times[i];
        result.skipped.extend(level.skipped.clone());
        let mut entry = MapEntry {
            geometry: Some(format!("map/{}/map.glb", level.stem)),
            views: None,
            kind: Some(level.kind),
            bsps: Some(level.bsps.len()),
        };
        let views: Vec<ViewEntry> = plan
            .into_iter()
            .map(|(e, _)| e)
            .filter(|e| !failed.contains_key(&format!("map/{}/views/{}", level.stem, e.id)))
            .collect();
        let file = ViewsFile {
            geometry_version: GEOMETRY_VERSION,
            map: level.stem.clone(),
            bounds: level.mesh.bounds,
            frame: level.frame,
            views,
        };
        let path = out.join("map").join(&level.stem).join("views.json");
        match serde_json::to_string_pretty(&file)
            .map_err(anyhow::Error::from)
            .and_then(|j| Ok(fs::write(&path, format!("{j}\n"))?))
        {
            Ok(()) if !file.views.is_empty() => {
                entry.views = Some(format!("map/{}/views.json", level.stem));
            }
            Ok(()) => {}
            Err(e) => {
                result
                    .skipped
                    .insert(format!("map/{}/views", level.stem), format!("{e:#}"));
            }
        }
        result.maps.insert(level.stem.clone(), entry);
        result.stats.push(stats);
    }
    result.skipped.extend(failed);
    result
}

#[cfg(test)]
#[path = "live_geometry_tests.rs"]
mod tests;
