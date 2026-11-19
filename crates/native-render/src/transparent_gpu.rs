//! Stage 3's GPU side: the world's transparent materials, each layer-15
//! pass compiled with its own programs ([`crate::program::compile_pass`])
//! and drawn with its own blend, depth and cull state
//! ([`crate::pass_state`]), one sorted entry at a time
//! (docs/graphics/native-renderer.md, "Stage 3: transparents").
//!
//! A material is drawn natively only when every pass uploads, and an entry
//! only when its part's lightmap page binds for every pass: anything else
//! keeps its D3D draw (the host does not accept the entry).

use std::collections::{BTreeSet, HashMap};

use crate::gpu::{Image, texture};
use crate::look::Stage;
use crate::pass_state::{PassState, TransparentState};
use crate::program::{self, Gpu, MaterialProgram, RigidUniforms, ViewInputs};
use gfx::state::RenderState;
use shaders::vsh::{VertexOptions, VertexUniforms};
use crate::world::{LightmapKey, Owner, TransparentDraw, World};

/// One compiled layer-15 pass of a material.
#[derive(Clone, Debug)]
pub struct TransparentProgram {
    pub program: MaterialProgram,
    pub state: TransparentState,
    pub words: Vec<u32>,
}

impl TransparentProgram {
    pub fn compile(pass: &crate::look::TransparentPass, code: &[u8]) -> Result<Self, String> {
        let look = crate::look::WorldPass {
            look: pass.look.clone(),
            state: pass.state.resolve(&RenderState::default()),
            inherits_cull: false,
        };
        let program = program::compile_world(&look, code)?;
        let parsed = shaders::vsh::Program::parse(code).map_err(|e| format!("vertex program: {e:?}"))?;
        let words = program::instruction_words(code, &parsed);
        Ok(Self { program, state: pass.state.clone(), words })
    }
}

/// The placement classification has a different program from the cluster
/// classification, even when the two use the same material.
pub type Programs = HashMap<(u16, SectionKind), Vec<TransparentProgram>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionKind {
    Cluster,
    Placement,
}

impl From<Owner> for SectionKind {
    fn from(owner: Owner) -> Self {
        match owner {
            Owner::Cluster(_) => Self::Cluster,
            Owner::Instance(_) => Self::Placement,
        }
    }
}

struct Material {
    /// The passes in draw order.
    passes: Vec<(Gpu, TransparentState, Vec<u32>)>,
    /// Per lightmap page the material's parts use: one texture group per
    /// pass.
    groups: HashMap<Option<LightmapKey>, Vec<wgpu::BindGroup>>,
}

/// The transparent materials the native pass draws.
pub struct TransparentSet {
    materials: HashMap<(u16, SectionKind), Material>,
    lightmap_info: HashMap<LightmapKey, String>,
}

impl TransparentSet {
    /// Uploads `programs` (each material's passes in order) for `world`'s
    /// transparent parts. `stage_image` decodes a stage's bitmap image,
    /// `lightmap` a page.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        programs: Programs,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let mut lightmap_info = HashMap::new();
        let bc = device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let mut materials = HashMap::new();
        'material: for (key, passes) in programs {
            if passes.is_empty() {
                continue;
            }
            let mut gpus = Vec::with_capacity(passes.len());
            for p in passes {
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                gpus.push((g, p.state, p.words));
            }
            let keys: BTreeSet<Option<LightmapKey>> = world
                .transparents
                .iter()
                .filter(|t| (t.material, t.owner.into()) == key)
                .map(|t| t.lightmap)
                .collect();
            let mut groups = HashMap::new();
            for page_key in keys {
                let page = match page_key {
                    Some(k) => pages
                        .entry(k)
                        .or_insert_with(|| lightmap(k).map(|i| {
                            lightmap_info.insert(k, program::describe_image(&i, bc));
                            texture(device, queue, &i, "native transparent lightmap")
                        }))
                        .clone(),
                    None => None,
                };
                let per_pass: Option<Vec<wgpu::BindGroup>> =
                    gpus.iter().map(|(g, _, _)| g.textures(device, page.as_ref())).collect();
                if let Some(g) = per_pass {
                    groups.insert(page_key, g);
                }
            }
            materials.insert(key, Material { passes: gpus, groups });
        }
        Self { materials, lightmap_info }
    }

    /// How many material/classification pairs are drawn natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Draw state and constants for each supported pass of this entry.
    pub fn uniforms(
        &self,
        world: &World,
        t: &TransparentDraw,
        view: &ViewInputs,
        incoming: &RenderState,
    ) -> Vec<(RigidUniforms, PassState)> {
        let Some(m) = self.materials.get(&(t.material, t.owner.into())) else { return Vec::new() };
        if !m.groups.contains_key(&t.lightmap) {
            return Vec::new();
        }
        m.passes.iter().map(|(g, state, words)| {
            let view = state.view(view);
            let state = state.resolve(incoming);
            let p = &g.program;
            (RigidUniforms {
                material: t.material,
                words: words.clone(),
                vertex: vertex(world, t.owner, &p.vertex, &view),
                pixel: program::frame_pixel(&p.pixel, &view),
                options: VertexOptions { provided_inputs: program::INPUT_MASK, input_fetch: program::WORLD_FETCH, ..Default::default() },
                def: p.def.clone(),
                pixel_options: p.pixel_options,
                samplers: std::array::from_fn(|n| p.dims[n].and(p.samplers[n].as_ref()).and_then(program::sampler_key)),
                textures: std::array::from_fn(|n| {
                    if p.stages[n] == Stage::Lightmap {
                        t.lightmap.and_then(|key| self.lightmap_info.get(&key)).cloned()
                    } else {
                        g.stage_info[n].clone()
                    }
                }),
                pipeline: program::pipeline_state(Some(&state), None),
            }, state)
        }).collect()
    }

    /// Prepares an entry's passes in order, with the section's unplaced
    /// vertices and the model rows a placement's classification expects.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        formats: (wgpu::TextureFormat, wgpu::TextureFormat),
        world: &World,
        t: &TransparentDraw,
        view: &ViewInputs,
        incoming: &RenderState,
    ) -> Vec<Draw> {
        let Some(m) = self.materials.get_mut(&(t.material, t.owner.into())) else { return Vec::new() };
        let Some(groups) = m.groups.get(&t.lightmap) else { return Vec::new() };
        m.passes.iter_mut().zip(groups).map(|((g, state, _), textures)| {
            let view = state.view(view);
            let state = state.resolve(incoming);
            let vertex = vertex(world, t.owner, &g.program.vertex, &view);
            let pixel = program::frame_pixel(&g.program.pixel, &view);
            let uniforms = g.draw_uniforms(device, &vertex, &pixel);
            let pipeline = g.pipeline(device, formats.0, formats.1, state).clone();
            Draw { pipeline, uniforms, textures: textures.clone(), blend: state.blend_constant }
        }).collect()
    }
}

fn vertex(world: &World, owner: Owner, base: &VertexUniforms, view: &ViewInputs) -> VertexUniforms {
    match owner {
        Owner::Cluster(_) => program::frame_vertex(base, view),
        Owner::Instance(i) => program::rigid_vertex(base, view, world.rows(i).expect("accepted placement has model rows")),
    }
}

pub(crate) struct Draw {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    blend: wgpu::Color,
}

impl Draw {
    pub(crate) fn encode(&self, pass: &mut wgpu::RenderPass<'_>, t: &TransparentDraw) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniforms, &[]);
        pass.set_bind_group(1, &self.textures, &[]);
        pass.set_blend_constant(self.blend);
        pass.draw_indexed(t.first_index..t.first_index + t.index_count, 0, 0..1);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_placement_uniforms_use_the_placement_rows_and_device_clip() {
        let rows = [[1.0, 0.0, 0.0, 10.0], [0.0, 1.0, 0.0, 20.0], [0.0, 0.0, 1.0, 30.0]];
        let world = World { placements: vec![(4, rows)], ..Default::default() };
        let view = ViewInputs {
            constants: [[2.0; 4]; program::VIEW_CONSTANTS],
            guest_size: (64, 64),
            size: (64, 64),
            z_scale: 1024.0,
            w_clip: Some(([3.0, 777.0], true)),
        };
        let base = VertexUniforms::default();
        let instance = vertex(&world, Owner::Instance(4), &base, &view);
        let cluster = vertex(&world, Owner::Cluster(4), &base, &view);
        let model = program::MODEL_ROWS..program::MODEL_ROWS + 3;
        assert_eq!(instance.constants[model.clone()], rows);
        assert_eq!(cluster.constants[model.clone()], base.constants[model]);
        assert_eq!(instance.constants[..program::VIEW_CONSTANTS], view.constants);
        assert_eq!(instance.w_clip, Some([3.0, 777.0]));
        assert!(instance.w_clamp);
    }
}
