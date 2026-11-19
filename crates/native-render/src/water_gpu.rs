//! The water edge blend, layer 6 (docs/graphics/native-renderer.md,
//! "Stage 4b: water and the rest"): the water templates' own layer-6
//! passes, drawn natively inside the window between the game's water
//! setup `0x0003ba90` and its teardown `0x0003c3c0` (whose alpha-only
//! clear `0x3c515` stays in guest order — the wrappers are never
//! skipped, only the layer's items).
//!
//! What makes water its own module beside [`crate::fog_gpu`] is the
//! stages: a layer-6 pass binds engine textures — the scene itself —
//! which no load can decode. Each is supplied per view ([`Supply`]):
//! the view's own target (id 5) as a copy of the bound colour made in
//! the recording ([`WaterSet::make_copy`]: an attachment cannot be
//! sampled as it is drawn into), the render targets through the
//! reservation's sampled levels (their guest surface headers from
//! Halo's target table `0x004b4b58`, boot's glue). The look keeps only
//! those two kinds ([`crate::fog_gpu::supplied_layer_passes`]);
//! anything else refuses the material, and the census names what a
//! level binds.
//!
//! The geometry is the water surfaces themselves, the kind-4 transparent
//! parts ([`crate::world::World::transparents`]): one item draws them in
//! layer 15 and again in layer 6, and only layer 6 is taken here.

use std::collections::HashMap;

use crate::fog_gpu::{supplied_layer_passes, FogPass, FogProgram, FogWords};
use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::pass_state::PassState;
use crate::program::{Gpu, ViewInputs};
use crate::world::{LightmapKey, Owner};

/// The layer this module draws.
pub const WATER_LAYER: usize = 6;

/// How one engine-texture stage is supplied per view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Supply {
    /// Engine texture 5, the view's bound target: a copy of the bound
    /// colour made in the recording ([`WaterSet::make_copy`]), before
    /// the pass draws into it.
    View,
    /// A render target by index (the engine-texture table's `Target`):
    /// the reservation's sampled level of its guest surface header
    /// (`native_water_glue::target_header`).
    Target(u32),
}

/// A stage's supply, when it binds an engine texture (`Supply` of the id
/// `crate::screen_look::engine_texture` maps).
pub fn supply_of(stage: Stage) -> Option<Supply> {
    match stage {
        Stage::Engine(id) => match crate::screen_look::engine_texture(id) {
            crate::screen_look::EngineTexture::View => Some(Supply::View),
            crate::screen_look::EngineTexture::Target(t) => Some(Supply::Target(t)),
            _ => None,
        },
        _ => None,
    }
}

/// The water materials the native pass draws, their parts' index ranges,
/// and the copy of the bound colour the view stages sample.
pub struct WaterSet {
    materials: HashMap<u16, WaterMaterial>,
    /// Each water part, by owner and part index: its index range,
    /// lightmap page and material, in the transparents' order.
    parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
    /// The distinct render targets the materials sample (the supplies'
    /// `Target`s), ascending: the order of the sampled levels the
    /// reservation hands [`WaterSet::draw`].
    pub targets: Vec<u32>,
    /// Whether any material samples the view's own target.
    pub samples_view: bool,
    /// The lightmap pages the parts use, as views.
    pages: HashMap<LightmapKey, Option<wgpu::TextureView>>,
    /// The kept copy of the bound colour (extent, format, texture, view),
    /// remade when the target changes.
    copy: Option<(wgpu::Extent3d, wgpu::TextureFormat, wgpu::Texture, wgpu::TextureView)>,
}

struct WaterMaterial {
    passes: Vec<(Gpu, PassState)>,
    /// Per stage: the engine supply ([`supply_of`]).
    supplies: [Option<Supply>; 4],
    words: FogWords,
}

impl WaterSet {
    /// Uploads `programs` (each material's layer-6 passes and their
    /// compiled programs) for `parts` (the transparents' parts list:
    /// owner and part, index range, lightmap page and material).
    /// `stage_image` decodes a stage's bitmap image, `lightmap_image` a
    /// page. A material is kept only when every pass uploads and its
    /// passes agree on their supplies.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
        programs: HashMap<u16, (Vec<FogPass>, Vec<FogProgram>)>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let mut materials = HashMap::new();
        'material: for (material, (passes, compiled)) in programs {
            if passes.is_empty() || passes.len() != compiled.len() {
                continue;
            }
            let supplies = passes[0].look.textures.map(supply_of);
            if passes.iter().any(|p| p.look.textures.map(supply_of) != supplies) {
                continue; // the passes disagree on their supplies
            }
            let words = compiled[0].words;
            if compiled.iter().any(|p| p.words != words) {
                continue;
            }
            let mut gpus = Vec::with_capacity(compiled.len());
            for p in compiled {
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                gpus.push((g, p.state));
            }
            materials.insert(material, WaterMaterial { passes: gpus, supplies, words });
        }
        let mut targets: Vec<u32> = materials
            .values()
            .flat_map(|m| {
                m.supplies
                    .iter()
                    .filter_map(|s| match s { Some(Supply::Target(t)) => Some(*t), _ => None })
            })
            .collect();
        targets.sort_unstable();
        targets.dedup();
        let samples_view = materials.values().any(|m| m.supplies.contains(&Some(Supply::View)));
        let mut pages = HashMap::new();
        for &(_, (_, _, lightmap, material)) in &parts {
            if materials.contains_key(&material)
                && let Some(k) = lightmap
                && !pages.contains_key(&k)
            {
                pages.insert(k, lightmap_image(k).map(|i| texture(device, queue, &i, "native water lightmap")));
            }
        }
        Self { materials, parts, targets, samples_view, pages, copy: None }
    }

    /// How many materials draw natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Whether the set draws `part` of `owner`: the `prepare_copy` check
    /// that every selected part will record.
    pub fn draws_part(&self, owner: Owner, part: u8) -> bool {
        self.parts.iter().any(|&((o, p), _)| o == owner && p == part)
    }

    /// The fog words of `material`'s passes, to resolve over the device's
    /// (`FogWords::resolve`): a water pass that reads the fog register
    /// gets the device's fog like a fog pass does.
    pub fn words(&self, material: u16) -> Option<&FogWords> {
        self.materials.get(&material).map(|m| &m.words)
    }

    /// The frame's uniforms for every pass (each material's fog resolved
    /// over the device's words, as the fog set's), through the
    /// submission's upload arena.
    pub fn prepare(
        &self,
        uploads: &mut gfx::ring::EncoderUploads<'_>,
        view: &ViewInputs,
        fogs: &HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    ) {
        for (&material, m) in &self.materials {
            let Some((fog, color)) = fogs.get(&material) else { continue };
            for (g, _) in &m.passes {
                let mut pixel = g.program.pixel;
                pixel.fog = fog.uniform();
                pixel.fog_color = *color;
                g.prepare_with(uploads, view, &pixel);
            }
        }
    }

    /// Makes the pipelines for these target formats (before the pass).
    pub fn pipelines(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat, depth: wgpu::TextureFormat) {
        for m in self.materials.values_mut() {
            for (g, state) in &mut m.passes {
                g.pipeline(device, color, depth, Some(&*state));
            }
        }
    }

    /// Copies the bound colour for the stages that sample the view's own
    /// target, into the kept copy (remade when the target's extent or
    /// format changes), and returns its view. The copy is made before
    /// the pass draws into the target, so the stages read the scene as
    /// the wrapper's own draws found it.
    pub fn make_copy(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView) -> Option<&wgpu::TextureView> {
        let texture = color.texture();
        let extent = wgpu::Extent3d { width: texture.width(), height: texture.height(), depth_or_array_layers: 1 };
        let format = texture.format();
        if self.copy.as_ref().is_none_or(|(e, f, _, _)| *e != extent || *f != format) {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native water view copy"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            });
            let v = t.create_view(&Default::default());
            self.copy = Some((extent, format, t, v));
        }
        let (_, _, source, copy) = self.copy.as_ref()?;
        encoder.copy_texture_to_texture(source.as_image_copy(), copy.as_image_copy(), extent);
        Some(copy)
    }

    /// Records every part's passes in order into `pass` (the world's
    /// vertex and index buffers bound): `copy` the view copy
    /// ([`WaterSet::make_copy`], when a material samples the view), and
    /// `sampled` the render targets' views in [`WaterSet::targets`]'
    /// order. Returns how many parts were drawn; a part whose supply has
    /// no view records nothing (its material was refused at the ask).
    pub fn draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        copy: Option<&wgpu::TextureView>,
        sampled: &[Option<wgpu::TextureView>],
    ) -> usize {
        let mut drawn = 0;
        let mut groups: HashMap<(u16, Option<LightmapKey>), Vec<wgpu::BindGroup>> = HashMap::new();
        for &(_, (first_index, index_count, lightmap, material)) in &self.parts {
            let Some(m) = self.materials.get(&material) else { continue };
            let page = match lightmap {
                Some(k) => self.pages.get(&k).cloned().flatten(),
                None => None,
            };
            let key = (material, lightmap);
            if !groups.contains_key(&key) {
                // The per-view views: the supplies' stages.
                let mut supplied: HashMap<usize, wgpu::TextureView> = HashMap::new();
                let mut ok = true;
                for (stage, s) in m.supplies.iter().enumerate() {
                    match s {
                        Some(Supply::View) => {
                            if let Some(v) = copy {
                                supplied.insert(stage, v.clone());
                            } else {
                                ok = false;
                            }
                        }
                        Some(Supply::Target(t)) => match self.targets.iter().position(|&want| want == *t).and_then(|i| sampled.get(i)) {
                            Some(Some(view)) => {
                                supplied.insert(stage, view.clone());
                            }
                            _ => ok = false,
                        },
                        None => {}
                    }
                }
                if !ok {
                    groups.insert(key, Vec::new());
                    continue;
                }
                let views: Vec<Option<wgpu::BindGroup>> = m
                    .passes
                    .iter()
                    .map(|(g, _)| g.textures_supplied(device, page.as_ref(), &supplied))
                    .collect();
                match views.into_iter().collect::<Option<Vec<_>>>() {
                    Some(v) => {
                        groups.insert(key, v);
                    }
                    None => {
                        groups.insert(key, Vec::new());
                    }
                }
            }
            let Some(entry) = groups.get(&key) else { continue };
            if entry.is_empty() || m.passes.iter().any(|(g, _)| !g.pipelines.contains_key(&(color_format, depth_format))) {
                continue;
            }
            for ((g, state), textures) in m.passes.iter().zip(entry) {
                pass.set_pipeline(&g.pipelines[&(color_format, depth_format)]);
                pass.set_bind_group(0, &g.uniforms, &[]);
                pass.set_bind_group(1, textures, &[]);
                pass.set_blend_constant(state.blend_constant);
                pass.draw_indexed(first_index..first_index + index_count, 0, 0..1);
            }
            drawn += 1;
        }
        drawn
    }
}

/// The layer-6 passes of `material` with the per-view supplied engine
/// stages kept: [`supplied_layer_passes`] at [`WATER_LAYER`].
pub fn water_passes(material: &tags::material::Material) -> Result<Vec<FogPass>, crate::fog_gpu::FogRefusal> {
    supplied_layer_passes(material, WATER_LAYER)
}
