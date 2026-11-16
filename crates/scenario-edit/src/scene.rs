//! What the viewport draws when the map's art is at hand: the structure
//! BSP textured with each shader's base map, and the placed objects' render
//! models, read from the user's own map at run time. UI-free and GPU-free;
//! [`crate::render::Viewport::set_scene`] uploads it.
//!
//! - The base map of a shader is the first bitmap of its compiled
//!   postprocess whose tag name reads as diffuse ([`bitmap_role`]), the
//!   same rule as `cli::live_models` and `cli::live_looks` for the Live
//!   site's level art (docs/live/live-art.md, "Textures").
//! - Bitmaps are decoded by `assets::decode_image`, once each, shrunk to at
//!   most [`MAX_TEXTURE`] a side, with a box-filtered mip chain.
//! - Objects resolve through `tags::object` (object → `hlmt` → `mode`) and
//!   draw each region's first permutation at its best LOD
//!   (`RenderModelGeometry::default_sections`), in model space; the
//!   viewport places them by each placement's transform.
//!
//! - The BSP's lightmap pages ([`geometry::world_parts_lit`]) are decoded
//!   from `P8` through their group's palette ([`lightmap_page`]) and light
//!   the parts that have them; the rest, and the models, are lit by one
//!   directional light.
use crate::geometry::{
    self, Bounds, Marker, Ray, Vec3, WorldPart, add, dot, normalize, scale, sub,
};
use crate::layout::BlockKind;
use crate::save::MapSource;
use assets::{Region, ResourceFiles};
use map::{FourCC, Map, ResourceFile, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};
use tags::geometry::{RawSource, TagView};
use tags::model::RenderModelGeometry;

/// The largest side of an uploaded texture.
pub const MAX_TEXTURE: u32 = 512;
/// The mean linear brightness [`Image::lift_factor`] raises dark textures to.
pub const LIFT_TARGET: f32 = 0.16;
/// The most [`Image::lift_factor`] multiplies a texture by.
pub const MAX_LIFT: f32 = 3.0;
/// Placed models are skipped past this many triangles each (none on the
/// disc comes close; it bounds a hostile map).
pub const MAX_MODEL_TRIANGLES: usize = 200_000;

/// One vertex of a textured mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SceneVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Coordinates in the batch's lightmap page.
    pub lightmap_uv: [f32; 2],
}

/// How a batch is blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Blend {
    /// Solid: the texture's alpha (often a specular mask) is ignored.
    Opaque,
    /// Cut out where the texture's alpha is low (grates, leaves).
    Mask,
    /// Drawn see-through after everything solid, without depth writes.
    Translucent,
}

/// How a shader template draws its base map: see-through for templates in
/// a `transparent` or `water` folder and for plain `overlay` decals, cut
/// out when the name says `alpha_test`, else solid (after the rule of
/// `cli::live_looks`, but `tex_bump_detail_overlay`, Coagulation's rock,
/// is solid).
pub fn template_blend(template: &str) -> Blend {
    let lower = template.to_ascii_lowercase();
    let stem = lower.rsplit('\\').next().unwrap_or(&lower);
    if lower
        .split('\\')
        .any(|c| c == "transparent" || c == "water")
        || stem == "overlay"
    {
        Blend::Translucent
    } else if stem.contains("alpha_test") {
        Blend::Mask
    } else {
        Blend::Opaque
    }
}

/// What a shader's bitmap is for, from the words of its tag name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitmapRole {
    Diffuse,
    Other,
}

/// One part of art read outside a Halo 2 map (a Halo 1 BSP) for
/// [`reference_scene`]: its geometry and what it draws with.
#[derive(Clone, Debug)]
pub struct ReferencePart {
    pub part: WorldPart,
    /// Index into the scene's textures.
    pub texture: Option<usize>,
    /// Index into the scene's lightmap pages.
    pub lightmap: Option<usize>,
    pub blend: Blend,
}

/// A scene from pictures and parts read outside a Halo 2 map, drawn as a
/// Halo 2 BSP is: base maps fitted, mipmapped and lifted like
/// [`SceneBuilder`]'s (those a [`Blend::Mask`] part uses have their alpha
/// stretched), lightmap pages unlifted. `shaders` and `textured` are for
/// the status bar.
pub fn reference_scene(
    textures: Vec<Image>,
    lightmaps: Vec<Image>,
    parts: &[ReferencePart],
    shaders: usize,
    textured: usize,
) -> Scene {
    let mut scene = Scene::default();
    let cutouts: std::collections::HashSet<usize> = parts
        .iter()
        .filter(|p| p.blend == Blend::Mask)
        .filter_map(|p| p.texture)
        .collect();
    for (i, mut image) in textures.into_iter().enumerate() {
        if cutouts.contains(&i) {
            image.stretch_alpha();
        }
        let image = image.fit(MAX_TEXTURE);
        scene.lifts.push(image.lift_factor());
        scene.textures.push(image);
    }
    let pages = scene.textures.len();
    scene.stats.lightmap_pages = lightmaps.len();
    for page in lightmaps {
        scene.lifts.push(1.0);
        scene.textures.push(page);
    }
    for p in parts {
        let lightmap = p.lightmap.map(|l| pages + l);
        scene.world.push(p.texture, lightmap, p.blend, &p.part);
    }
    scene.stats.shaders = shaders;
    scene.stats.textured = textured;
    scene
}

/// The rule of `cli::live_models::bitmap_role`, reduced to "is this the
/// diffuse map": not an engine, change-colour, self-illumination, bump,
/// detail, reflection or mask map.
pub fn bitmap_role(name: &str) -> BitmapRole {
    let lower = name.to_ascii_lowercase();
    let last = lower.rsplit('\\').next().unwrap_or(&lower);
    let folders = [
        "default_bitmaps",
        "rasterizer",
        "detail_maps",
        "reflection_maps",
        "cube",
    ];
    let words = [
        "_cc",
        "change_color",
        "change_colour",
        "color_change",
        "colour_change",
        "bump",
        "_bmp",
        "detail",
        "reflection",
        "cube",
        "fade",
        "normal",
        "illum",
        "self",
        "specular",
        "mask",
    ];
    if lower.split('\\').any(|c| folders.contains(&c))
        || last.ends_with("_cc")
        || words.iter().any(|w| last.contains(w))
    {
        BitmapRole::Other
    } else {
        BitmapRole::Diffuse
    }
}

/// An RGBA8 picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Half the size (each side at least 1), box filtered.
    pub fn half(&self) -> Image {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let at = |x: u32, y: u32, c: u32| -> u32 {
            let x = x.min(self.width - 1);
            let y = y.min(self.height - 1);
            u32::from(self.rgba[((y * self.width + x) * 4 + c) as usize])
        };
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    let (sx, sy) = (x * 2, y * 2);
                    let sum = at(sx, sy, c)
                        + at(sx + 1, sy, c)
                        + at(sx, sy + 1, c)
                        + at(sx + 1, sy + 1, c);
                    rgba[((y * w + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    /// Halved until neither side exceeds `max`.
    pub fn fit(mut self, max: u32) -> Image {
        while self.width > max || self.height > max {
            self = self.half();
        }
        self
    }

    /// This image and its mips down to 1x1.
    pub fn mip_chain(self) -> Vec<Image> {
        let mut out = vec![self];
        while let Some(last) = out.last()
            && (last.width > 1 || last.height > 1)
        {
            let next = last.half();
            out.push(next);
        }
        out
    }

    /// How much to brighten it (at most [`MAX_LIFT`]) for its mean to be at
    /// least [`LIFT_TARGET`] in linear light, when it is not lightmapped. The game draws base maps under
    /// detail maps that double them and under bright lightmaps; drawn
    /// alone by one sun, dark metals (Ascension's Forerunner panels, a
    /// mean near 0.06) read as black.
    pub fn lift_factor(&self) -> f32 {
        let linear: [f32; 256] = std::array::from_fn(|c| (c as f32 / 255.0).powf(2.2));
        let n = (self.rgba.len() / 4).max(1) as f32;
        let mean = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (linear[p[0] as usize] + linear[p[1] as usize] + linear[p[2] as usize]) / 3.0)
            .sum::<f32>()
            / n;
        if mean <= 0.0 || mean >= LIFT_TARGET {
            return 1.0;
        }
        (LIFT_TARGET / mean).min(MAX_LIFT)
    }

    /// Alpha stretched so the strongest is opaque, then doubled: a cut at
    /// a half keeps what is at least a quarter of the strongest (leaves
    /// keep little alpha at a half; `cli::live_looks::cut_at_a_quarter`).
    pub fn stretch_alpha(&mut self) {
        let max = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .max()
            .unwrap_or(0);
        if max == 0 {
            return;
        }
        for px in self.rgba.as_chunks_mut::<4>().0 {
            px[3] = (u32::from(px[3]) * 510 / u32::from(max)).min(255) as u8;
        }
    }
}

/// Triangles sharing one texture and blend.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    /// Index into [`Scene::textures`]; `None` draws untextured grey.
    pub texture: Option<usize>,
    /// The lightmap page lighting it, an index into [`Scene::textures`];
    /// `None` is lit by the sun.
    pub lightmap: Option<usize>,
    pub blend: Blend,
    pub vertices: Vec<SceneVertex>,
    pub indices: Vec<u32>,
}

/// Batches in one space (the world, or one model's).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub batches: Vec<Batch>,
    pub bounds: Bounds,
}

impl Default for Mesh {
    fn default() -> Self {
        Self {
            batches: Vec::new(),
            bounds: Bounds::EMPTY,
        }
    }
}

impl Mesh {
    /// Add one part's triangles under `(texture, blend)`, merged into the
    /// batch with that key. Triangles with out-of-range indices are dropped.
    pub fn push(
        &mut self,
        texture: Option<usize>,
        lightmap: Option<usize>,
        blend: Blend,
        part: &WorldPart,
    ) {
        // Lightmap coordinates are only meaningful with the part's own page.
        let lightmap = lightmap.filter(|_| part.lightmap_uvs.len() == part.positions.len());
        let i = match self
            .batches
            .iter()
            .position(|b| b.texture == texture && b.lightmap == lightmap && b.blend == blend)
        {
            Some(i) => i,
            None => {
                self.batches.push(Batch {
                    texture,
                    lightmap,
                    blend,
                    vertices: Vec::new(),
                    indices: Vec::new(),
                });
                self.batches.len() - 1
            }
        };
        let b = &mut self.batches[i];
        let base = b.vertices.len() as u32;
        for (k, &p) in part.positions.iter().enumerate() {
            if blend != Blend::Translucent {
                self.bounds.add(p);
            }
            b.vertices.push(SceneVertex {
                position: p,
                normal: part.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]),
                uv: part.uvs.get(k).copied().unwrap_or([0.0; 2]),
                lightmap_uv: part.lightmap_uvs.get(k).copied().unwrap_or([0.0; 2]),
            });
        }
        let n = part.positions.len() as u32;
        for t in &part.tris {
            if t.iter().all(|&i| i < n) {
                b.indices.extend(t.map(|i| base + i));
            }
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }

    /// Distance along `ray` to the nearest triangle that is not see-through.
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        let mut best: Option<f32> = None;
        for b in self
            .batches
            .iter()
            .filter(|b| b.blend != Blend::Translucent)
        {
            for t in b.indices.as_chunks::<3>().0 {
                let p = [0, 1, 2].map(|k| b.vertices[t[k] as usize].position);
                if let Some(d) = ray.triangle(&p)
                    && best.is_none_or(|b| d < b)
                {
                    best = Some(d);
                }
            }
        }
        best
    }
}

/// Counts for the status bar and the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub shaders: usize,
    /// Shaders drawn with a base map.
    pub textured: usize,
    pub bitmaps_failed: usize,
    pub objects: usize,
    /// Objects drawn with a model.
    pub modelled: usize,
    pub models_failed: usize,
    /// Lightmap pages decoded.
    pub lightmap_pages: usize,
}

/// A map's drawable art.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub textures: Vec<Image>,
    /// Per texture, how much the sun-lit shading brightens it
    /// ([`Image::lift_factor`]); 1 for lightmap pages. Lightmapped
    /// surfaces are not lifted.
    pub lifts: Vec<f32>,
    pub world: Mesh,
    pub models: Vec<Mesh>,
    /// Placed object tag → index into [`Self::models`].
    pub objects: HashMap<TagId, usize>,
    pub stats: SceneStats,
}

impl Scene {
    /// The model a placement draws, if any.
    pub fn model_of(&self, m: &Marker) -> Option<usize> {
        m.object.and_then(|o| self.objects.get(&o).copied())
    }

    /// The nearest placement whose model (or glyph) the ray hits.
    pub fn pick(&self, markers: &[Marker], ray: &Ray) -> Option<(BlockKind, usize)> {
        let mut best = geometry::pick_glyph(markers, ray);
        for m in markers {
            let Some(model) = self.model_of(m).and_then(|i| self.models.get(i)) else {
                continue;
            };
            if model.bounds.is_empty() {
                continue;
            }
            // Broad phase on the model's bounding sphere, placed.
            let c = model_to_world(m, model.bounds.center());
            if ray.sphere(c, model.bounds.radius() * m.scale).is_none() {
                continue;
            }
            // The ray in model space: axes are orthonormal, scaled by `scale`.
            let local = sub(ray.origin, m.position);
            let s = m.scale.max(1e-6);
            let to_local = |v: Vec3| m.axes.map(|a| dot(v, a));
            let lray = Ray {
                origin: scale(to_local(local), 1.0 / s),
                dir: to_local(ray.dir),
            };
            if let Some(t) = model.hit(&lray) {
                let t = t * s;
                if best.is_none_or(|b| t < b.0) {
                    best = Some((t, m.block, m.index));
                }
            }
        }
        best.map(|(_, b, i)| (b, i))
    }
}

/// A model-space point placed by a marker's transform.
pub fn model_to_world(m: &Marker, p: Vec3) -> Vec3 {
    let [f, l, u] = m.axes;
    let s = m.scale;
    add(
        m.position,
        add(
            scale(f, p[0] * s),
            add(scale(l, p[1] * s), scale(u, p[2] * s)),
        ),
    )
}

/// The files a map's raw pointers select, for `assets::decode_image`: the
/// map itself and the shared caches beside it. A map held in memory (a
/// package resolved over its base, or a converted map) is read from those
/// bytes: its local raw offsets are its own, and the base map on disk (for a
/// converted Halo 1 map, Coagulation or Beaver Creek) has other data there.
pub fn resource_files(source: &MapSource) -> ResourceFiles {
    let mut files = ResourceFiles::new();
    let region = |s: &MapSource| -> Option<Region> {
        let (base, length) = match s.region {
            Some(r) => r,
            None => (0, std::fs::metadata(&s.path).ok()?.len()),
        };
        Some(Region {
            path: s.path.clone(),
            base,
            length,
        })
    };
    let mut set = |file, s: &MapSource| {
        if let Some(r) = region(s) {
            let _ = files.set(file, r);
        }
    };
    if source.bytes.is_none() {
        set(ResourceFile::Local, &source.base());
    }
    for (file, s) in geometry::shared_sources(source) {
        set(file, &s);
    }
    if let Some(b) = &source.bytes {
        files.set_bytes(ResourceFile::Local, b.clone());
    }
    files
}

/// Builds a [`Scene`]: shaders read once, bitmaps decoded once.
pub struct SceneBuilder<'a, R> {
    map: &'a mut Map<R>,
    raw: &'a mut dyn RawSource,
    files: ResourceFiles,
    scene: Scene,
    /// Shader → (texture, blend).
    looks: HashMap<TagId, (Option<usize>, Blend)>,
    /// Bitmap → texture.
    bitmaps: HashMap<(TagId, bool), Option<usize>>,
}

impl<'a, R: Read + Seek> SceneBuilder<'a, R> {
    pub fn new(map: &'a mut Map<R>, raw: &'a mut dyn RawSource, files: ResourceFiles) -> Self {
        Self {
            map,
            raw,
            files,
            scene: Scene::default(),
            looks: HashMap::new(),
            bitmaps: HashMap::new(),
        }
    }

    /// The textured world from [`geometry::world_parts_lit`], lit by its
    /// lightmap pages where it has them.
    pub fn world(&mut self, parts: &[WorldPart], lightmaps: &[geometry::LightmapPages]) {
        let mut pages: HashMap<geometry::LightmapRef, Option<usize>> = HashMap::new();
        let mut bitmaps: HashMap<TagId, Option<Box<tags::Bitmap>>> = HashMap::new();
        for part in parts {
            let (texture, mut blend) = self.look(part.shader);
            if part.transparent {
                blend = Blend::Translucent;
            }
            let lightmap = part.lightmap.and_then(|r| {
                *pages.entry(r).or_insert_with(|| {
                    let src = lightmaps.get(r.pages)?;
                    let bitmap = bitmaps
                        .entry(src.bitmap)
                        .or_insert_with(|| {
                            match tags::DecodedTag::read(self.map, src.bitmap).ok()?.data {
                                tags::TagData::Bitmap(b) => Some(b),
                                _ => None,
                            }
                        })
                        .as_ref()?;
                    let image = bitmap.images.get(usize::from(r.page))?;
                    let palette = src.palettes.get(usize::from(r.palette))?;
                    let page = lightmap_page(image, palette, &mut self.files)?;
                    self.scene.stats.lightmap_pages += 1;
                    self.scene.lifts.push(1.0);
                    self.scene.textures.push(page);
                    Some(self.scene.textures.len() - 1)
                })
            });
            self.scene.world.push(texture, lightmap, blend, part);
        }
    }

    /// A model for every placed object tag that resolves to one.
    pub fn objects(&mut self, objects: impl IntoIterator<Item = TagId>) {
        let ctx = match tags::object::Context::read(self.map) {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut by_mode: HashMap<TagId, Option<usize>> = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for object in objects {
            if !seen.insert(object) {
                continue;
            }
            self.scene.stats.objects += 1;
            let Some(mode) = render_model_of(&ctx, object) else {
                continue;
            };
            let model = match by_mode.get(&mode) {
                Some(m) => *m,
                None => {
                    let m = self.model(mode);
                    if m.is_none() {
                        self.scene.stats.models_failed += 1;
                    }
                    by_mode.insert(mode, m);
                    m
                }
            };
            if let Some(m) = model {
                self.scene.objects.insert(object, m);
                self.scene.stats.modelled += 1;
            }
        }
    }

    pub fn finish(self) -> Scene {
        self.scene
    }

    fn model(&mut self, mode: TagId) -> Option<usize> {
        let geo = RenderModelGeometry::read(self.map, mode, self.raw).ok()?;
        let shaders: Vec<TagId> = geo.materials.iter().map(|m| m.shader.id).collect();
        let mut mesh = Mesh::default();
        let mut tris = 0;
        for si in geo.default_sections() {
            let Some(section) = geo.sections.get(si) else {
                continue;
            };
            let v = &section.vertices;
            for part in section.geometry.parts.iter().filter(|p| p.is_drawn()) {
                let mut out = WorldPart {
                    shader: shaders
                        .get(usize::from(part.material))
                        .copied()
                        .unwrap_or(TagId::NULL),
                    transparent: part.kind == geometry::PART_TRANSPARENT,
                    ..WorldPart::default()
                };
                let mut remap = HashMap::new();
                for t in section.geometry.part_triangles(part) {
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
                            out.positions.push(v.positions[i]);
                            out.normals
                                .push(v.normals.get(i).map_or([0.0, 0.0, 1.0], |&n| normalize(n)));
                            out.uvs
                                .push(v.texcoords.get(i).copied().unwrap_or([0.0; 2]));
                            (out.positions.len() - 1) as u32
                        })
                    });
                    out.tris.push(tri);
                }
                tris += out.tris.len();
                if tris > MAX_MODEL_TRIANGLES {
                    return None;
                }
                if v.normals.is_empty() {
                    out.normals = geometry::vertex_normals(&out.positions, &out.tris);
                }
                let (texture, mut blend) = self.look(out.shader);
                if out.transparent {
                    blend = Blend::Translucent;
                }
                mesh.push(texture, None, blend, &out);
            }
        }
        if mesh.batches.is_empty() {
            return None;
        }
        self.scene.models.push(mesh);
        Some(self.scene.models.len() - 1)
    }

    /// A shader's base map and blend, read the first time it is asked for.
    fn look(&mut self, shader: TagId) -> (Option<usize>, Blend) {
        if shader == TagId::NULL {
            return (None, Blend::Opaque);
        }
        if let Some(l) = self.looks.get(&shader) {
            return *l;
        }
        self.scene.stats.shaders += 1;
        let look = match shader_bitmaps(self.map, shader) {
            Ok((template, bitmaps)) => {
                let blend = template_blend(&template);
                let base = bitmaps.into_iter().find(|id| {
                    self.map
                        .tag(*id)
                        .ok()
                        .and_then(|t| t.name.as_deref())
                        .is_some_and(|n| bitmap_role(n) == BitmapRole::Diffuse)
                });
                let texture = base.and_then(|b| self.texture(b, blend == Blend::Mask));
                (texture, blend)
            }
            Err(_) => (None, Blend::Opaque),
        };
        if look.0.is_some() {
            self.scene.stats.textured += 1;
        }
        self.looks.insert(shader, look);
        look
    }

    fn texture(&mut self, bitmap: TagId, cutout: bool) -> Option<usize> {
        if let Some(t) = self.bitmaps.get(&(bitmap, cutout)) {
            return *t;
        }
        let t = match decode_bitmap(self.map, &mut self.files, bitmap) {
            Some(mut image) => {
                if cutout {
                    image.stretch_alpha();
                }
                let image = image.fit(MAX_TEXTURE);
                self.scene.lifts.push(image.lift_factor());
                self.scene.textures.push(image);
                Some(self.scene.textures.len() - 1)
            }
            None => {
                self.scene.stats.bitmaps_failed += 1;
                None
            }
        };
        self.bitmaps.insert((bitmap, cutout), t);
        t
    }
}

/// A lightmap page as RGBA: its `P8` indices (which `assets` decodes as
/// `L8`, the index in each channel) through the group's palette of
/// `A8R8G8B8` entries.
pub fn lightmap_page(
    image: &tags::Image,
    palette: &[u32; 256],
    files: &mut ResourceFiles,
) -> Option<Image> {
    if image.format != tags::PixelFormat::P8 {
        return None;
    }
    let decoded = assets::decode_image(image, files).ok()?;
    let level = decoded.levels.into_iter().find(|l| l.face == 0)?;
    Some(Image {
        width: level.width,
        height: level.height,
        rgba: apply_palette(&level.rgba, palette),
    })
}

/// Replace each texel (an index in its red channel) by its palette entry.
pub fn apply_palette(indices: &[u8], palette: &[u32; 256]) -> Vec<u8> {
    indices
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let c = palette[usize::from(p[0])];
            [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
        })
        .collect()
}

/// An object's render model: object → `hlmt` → `mode`. An item or vehicle
/// collection (`itmc`, `vehc`, what netgame equipment names) stands for its
/// first entry.
pub fn render_model_of(ctx: &tags::object::Context, object: TagId) -> Option<TagId> {
    let group = ctx.catalog.get(object)?.group;
    let object = if group == FourCC(*b"itmc") || group == FourCC(*b"vehc") {
        let tag = ctx.decode(object).ok()?;
        tag.layers
            .first()?
            .elements("entries")
            .iter()
            .filter_map(|e| e.tag_ref("object"))
            .map(|r| r.id)
            .find(|id| *id != TagId::NULL)?
    } else {
        object
    };
    let resolved = ctx.resolve_object(object).ok()?;
    let mode = resolved.model?.render_model?;
    (mode.group == FourCC(*b"mode")).then_some(mode.id)
}

/// A shader's template name and bitmaps: `shad +0` is the template
/// reference, `shad +0x20` the compiled postprocess (one element), whose
/// `+4` block lists the bitmap parameters (0xc bytes, datum first;
/// docs/formats/shader-instances.md).
pub fn shader_bitmaps<R: Read + Seek>(
    map: &mut Map<R>,
    shader: TagId,
) -> crate::Result<(String, Vec<TagId>)> {
    let tag = map.tag(shader)?.clone();
    if tag.group != FourCC(*b"shad") {
        return Err(crate::Error::Invalid("not a shader"));
    }
    let address = tag
        .location
        .ok_or(crate::Error::Invalid("shader has no payload"))?
        .address;
    let bytes = map.read_tag_bytes(shader)?;
    let view = TagView::new(&bytes, address);
    let word = |b: &[u8], o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    };
    let template = word(&bytes, 4)
        .and_then(|id| map.tag(TagId(id)).ok())
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    let root = view.at(0, 0x54)?;
    let post = view.block(root, 0x20, 0x7c)?;
    let post = post
        .first()
        .ok_or(crate::Error::Invalid("no compiled postprocess"))?;
    let ids = view
        .block(post, 4, 0xc)?
        .iter()
        .filter_map(|b| word(b, 0).map(TagId))
        .filter(|id| *id != TagId::NULL)
        .collect();
    Ok((template, ids))
}

/// The first image of a bitmap tag, its largest decoded level, as RGBA.
pub fn decode_bitmap<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    id: TagId,
) -> Option<Image> {
    let bitmap = match tags::DecodedTag::read(map, id).ok()?.data {
        tags::TagData::Bitmap(b) => b,
        _ => return None,
    };
    let decoded = assets::decode_image(bitmap.images.first()?, files).ok()?;
    let level = decoded
        .levels
        .into_iter()
        .find(|l| l.face == 0 && l.depth <= 1)?;
    (level.rgba.len() == (level.width * level.height * 4) as usize).then_some(Image {
        width: level.width,
        height: level.height,
        rgba: level.rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference art: base maps first, then lightmap pages (unlifted), a
    /// part's page index shifted past the base maps, cut-out alpha
    /// stretched, and the counts carried to the stats.
    #[test]
    fn reference_parts_index_their_textures_and_pages() {
        let pic = |c: u8, a: u8| Image {
            width: 2,
            height: 2,
            rgba: [c, c, c, a].repeat(4),
        };
        let part = |lit: bool| WorldPart {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: if lit { vec![[0.5; 2]; 3] } else { Vec::new() },
            tris: vec![[0, 1, 2]],
            ..WorldPart::default()
        };
        let parts = [
            ReferencePart {
                part: part(true),
                texture: Some(1),
                lightmap: Some(0),
                blend: Blend::Mask,
            },
            ReferencePart {
                part: part(false),
                texture: Some(0),
                lightmap: Some(0),
                blend: Blend::Opaque,
            },
        ];
        let s = reference_scene(
            vec![pic(40, 255), pic(200, 64)],
            vec![pic(90, 255)],
            &parts,
            3,
            2,
        );
        assert_eq!(s.textures.len(), 3);
        assert_eq!(s.lifts[2], 1.0);
        assert_eq!(
            s.textures[1].rgba[3], 255,
            "the cut-out's alpha is stretched"
        );
        let lit = s
            .world
            .batches
            .iter()
            .find(|b| b.texture == Some(1))
            .unwrap();
        assert_eq!(lit.lightmap, Some(2));
        assert_eq!(lit.vertices[0].lightmap_uv, [0.5; 2]);
        // A part without lightmap coordinates is not lit by the page.
        let unlit = s
            .world
            .batches
            .iter()
            .find(|b| b.texture == Some(0))
            .unwrap();
        assert_eq!(unlit.lightmap, None);
        assert_eq!(
            (s.stats.shaders, s.stats.textured, s.stats.lightmap_pages),
            (3, 2, 1)
        );
    }

    #[test]
    fn templates_and_bitmap_names_choose_blend_and_base_map() {
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump"),
            Blend::Opaque
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump_alpha_test"),
            Blend::Mask
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\one_add"),
            Blend::Translucent
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump_detail_overlay"),
            Blend::Opaque
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\water\\water_pool"),
            Blend::Translucent
        );
        assert_eq!(
            bitmap_role("scenarios\\bitmaps\\a\\metal_panels"),
            BitmapRole::Diffuse
        );
        for other in [
            "scenarios\\bitmaps\\a\\metal_panels_bump",
            "scenarios\\bitmaps\\a\\panels_vertical_outdoor_bmp",
            "scenarios\\bitmaps\\detail_maps\\grain",
            "rasterizer\\x",
            "objects\\x\\body_cc",
            "objects\\x\\self_illum",
        ] {
            assert_eq!(bitmap_role(other), BitmapRole::Other, "{other}");
        }
    }

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn images_halve_fit_and_chain_to_one_texel() {
        let img = image(4, 2, |x, _| {
            if x < 2 {
                [0, 0, 0, 255]
            } else {
                [200, 100, 50, 255]
            }
        });
        let h = img.clone().half();
        assert_eq!((h.width, h.height), (2, 1));
        assert_eq!(&h.rgba, &[0, 0, 0, 255, 200, 100, 50, 255]);
        let fit = image(1024, 256, |_, _| [1; 4]).fit(512);
        assert_eq!((fit.width, fit.height), (512, 128));
        let chain = img.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|i| (i.width, i.height)).collect();
        assert_eq!(sizes, [(4, 2), (2, 1), (1, 1)]);
    }

    #[test]
    fn dark_textures_are_lifted_and_bright_ones_kept() {
        // 40 is 0.017 linear: tripled (the cap).
        assert_eq!(image(2, 1, |_, _| [40, 40, 40, 7]).lift_factor(), MAX_LIFT);
        let half = image(1, 1, |_, _| [97, 97, 97, 255]).lift_factor();
        assert!((half - 1.36).abs() < 0.05, "{half}");
        assert_eq!(image(1, 1, |_, _| [200, 180, 160, 255]).lift_factor(), 1.0);
    }

    #[test]
    fn lightmap_indices_go_through_the_palette() {
        let mut palette = [0u32; 256];
        palette[3] = 0xff10_2030;
        palette[255] = 0x80ff_eedd;
        let out = apply_palette(&[3, 3, 3, 255, 255, 255, 255, 255], &palette);
        assert_eq!(out, [0x10, 0x20, 0x30, 255, 0xff, 0xee, 0xdd, 255]);
    }

    #[test]
    fn cutout_alpha_is_stretched_to_its_strongest() {
        let mut img = image(2, 1, |x, _| [0, 0, 0, if x == 0 { 64 } else { 16 }]);
        img.stretch_alpha();
        assert_eq!((img.rgba[3], img.rgba[7]), (255, 127));
    }

    fn quad(transparent: bool) -> WorldPart {
        WorldPart {
            shader: TagId::NULL,
            transparent,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0; 2]; 4],
            tris: vec![[0, 1, 2], [1, 3, 2], [0, 1, 9]],
            ..WorldPart::default()
        }
    }

    #[test]
    fn meshes_merge_by_texture_and_blend_and_drop_bad_triangles() {
        let mut m = Mesh::default();
        m.push(Some(0), None, Blend::Opaque, &quad(false));
        m.push(Some(0), None, Blend::Opaque, &quad(false));
        m.push(None, None, Blend::Translucent, &quad(true));
        assert_eq!(m.batches.len(), 2);
        assert_eq!(m.batches[0].vertices.len(), 8);
        assert_eq!(m.batches[0].indices.len(), 12);
        assert_eq!(m.batches[0].indices[6..9], [4, 5, 6]);
        assert_eq!(m.triangle_count(), 6);
        assert_eq!(m.bounds.max, [1.0, 1.0, 0.0]);
        // See-through batches are not hit.
        let down = Ray {
            origin: [0.25, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(m.hit(&down), Some(5.0));
        let mut glass = Mesh::default();
        glass.push(None, None, Blend::Translucent, &quad(true));
        assert_eq!(glass.hit(&down), None);
        assert!(glass.bounds.is_empty());
    }

    #[test]
    fn models_are_picked_through_their_placement() {
        let mut mesh = Mesh::default();
        mesh.push(None, None, Blend::Opaque, &quad(false));
        let mut scene = Scene::default();
        scene.models.push(mesh);
        scene.objects.insert(TagId(7), 0);
        // Turned 90° about z, doubled, at (10, 0, 0): the quad covers
        // x in [8, 10], y in [0, 2].
        let marker = Marker {
            block: BlockKind::Scenery,
            index: 3,
            position: [10.0, 0.0, 0.0],
            axes: geometry::rotation([std::f32::consts::FRAC_PI_2, 0.0, 0.0]),
            scale: 2.0,
            object: Some(TagId(7)),
        };
        let ray = |x: f32, y: f32| Ray {
            origin: [x, y, 3.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(
            scene.pick(&[marker], &ray(8.5, 1.5)),
            Some((BlockKind::Scenery, 3))
        );
        assert_eq!(scene.pick(&[marker], &ray(11.5, 1.5)), None);
        let p = model_to_world(&marker, [1.0, 1.0, 0.0]);
        assert!(
            (p[0] - 8.0).abs() < 1e-5 && (p[1] - 2.0).abs() < 1e-5,
            "{p:?}"
        );
    }
}
