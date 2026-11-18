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
use crate::pass_state::PassState;
use crate::program::{Gpu, MaterialProgram, ViewInputs};
use crate::world::{LightmapKey, TransparentDraw, World};

/// One compiled layer-15 pass of a material.
#[derive(Clone, Debug)]
pub struct TransparentProgram {
    pub program: MaterialProgram,
    pub state: PassState,
}

struct Material {
    /// The passes in draw order.
    passes: Vec<(Gpu, PassState)>,
    /// Per lightmap page the material's parts use: one texture group per
    /// pass.
    groups: HashMap<Option<LightmapKey>, Vec<wgpu::BindGroup>>,
}

/// The transparent materials the native pass draws.
pub struct TransparentSet {
    materials: HashMap<u16, Material>,
}

impl TransparentSet {
    /// Uploads `programs` (each material's passes in order) for `world`'s
    /// transparent parts. `stage_image` decodes a stage's bitmap image,
    /// `lightmap` a page.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &World,
        programs: HashMap<u16, Vec<TransparentProgram>>,
        stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
        lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
    ) -> Self {
        let mut pages: HashMap<LightmapKey, Option<wgpu::TextureView>> = HashMap::new();
        let mut materials = HashMap::new();
        'material: for (material, passes) in programs {
            if passes.is_empty() {
                continue;
            }
            let mut gpus = Vec::with_capacity(passes.len());
            for p in passes {
                let Some(g) = Gpu::new(device, queue, p.program, stage_image) else {
                    continue 'material;
                };
                gpus.push((g, p.state));
            }
            let keys: BTreeSet<Option<LightmapKey>> = world
                .transparents
                .iter()
                .filter(|t| t.material == material)
                .map(|t| t.lightmap)
                .collect();
            let mut groups = HashMap::new();
            for key in keys {
                let page = match key {
                    Some(k) => pages
                        .entry(k)
                        .or_insert_with(|| lightmap(k).map(|i| texture(device, queue, &i, "native transparent lightmap")))
                        .clone(),
                    None => None,
                };
                let per_pass: Option<Vec<wgpu::BindGroup>> =
                    gpus.iter().map(|(g, _)| g.textures(device, page.as_ref())).collect();
                if let Some(g) = per_pass {
                    groups.insert(key, g);
                }
            }
            materials.insert(material, Material { passes: gpus, groups });
        }
        Self { materials }
    }

    /// How many materials are drawn natively.
    pub fn len(&self) -> usize {
        self.materials.len()
    }

    pub fn is_empty(&self) -> bool {
        self.materials.is_empty()
    }

    /// Whether the pass draws `t` (its material uploaded and its page binds).
    pub fn draws(&self, t: &TransparentDraw) -> bool {
        self.materials.get(&t.material).is_some_and(|m| m.groups.contains_key(&t.lightmap))
    }

    /// The frame's uniforms for every pass.
    pub fn prepare(&self, queue: &wgpu::Queue, view: &ViewInputs) {
        for m in self.materials.values() {
            for (g, _) in &m.passes {
                g.prepare(queue, view);
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

    /// Records `t`'s passes in order into `pass` (the world's vertex and
    /// index buffers bound). `false`: nothing recorded.
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        t: &TransparentDraw,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> bool {
        let Some(m) = self.materials.get(&t.material) else { return false };
        let Some(groups) = m.groups.get(&t.lightmap) else { return false };
        if m.passes.iter().any(|(g, _)| !g.pipelines.contains_key(&(color, depth))) {
            return false;
        }
        for ((g, state), textures) in m.passes.iter().zip(groups) {
            pass.set_pipeline(&g.pipelines[&(color, depth)]);
            pass.set_bind_group(0, &g.uniforms, &[]);
            pass.set_bind_group(1, textures, &[]);
            pass.set_blend_constant(state.blend_constant);
            pass.draw_indexed(t.first_index..t.first_index + t.index_count, 0, 0..1);
        }
        true
    }
}
