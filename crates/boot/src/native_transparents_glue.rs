//! The native transparents' host (docs/graphics/native-renderer.md,
//! "Stage 3: transparents"): over the native world's loaded BSP
//! (`native_world_glue`, which loads it at the first layer 3 of a map), it
//! accepts the sorted transparent entries whose part and material the
//! native pass draws, and asks the D3D device for one external pass per
//! run of them (`feature::native_transparents`). Inert unless
//! `impl.render.native_transparents` is on (nothing is compiled at load
//! and the hooks are not placed); it needs the native world on, which
//! loads the BSP.
//!
//! - **At load** ([`load`], on the world's worker): each transparent
//!   material's layer-15 passes (`native_render::look::transparent_passes`)
//!   compiled with their own programs (`native_render::program::compile_pass`),
//!   all or nothing, and their stage images decoded; then the parts that
//!   can draw: a material that compiled, and its lightmap page when a pass
//!   samples it. One line says how many and why the rest stay on the D3D
//!   path.
//! - **Per entry** ([`Host::accepts`]): BSP clusters and instanced
//!   geometry only (objects stay on the D3D path), a part in that table
//!   whose shader is the entry's.
//! - **Per run** ([`Host::open`]): one external pass. The device issues it
//!   at its next entry; it takes the run's entries then
//!   (`ExternalPass::view_constants`, which seals the run) and draws them
//!   in order (`GpuWorld::encode_transparents`).

use std::collections::HashMap;
use std::sync::Arc;

use engine::memory::GuestMemory;
use feature::native_transparents::Host;
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::objects::ObjectFrame;
use native_render::program::{self, ViewInputs};
use native_render::transparent::{Entry, Run, Source};
use native_render::transparent_gpu::TransparentProgram;
use native_render::world::{LightmapKey, Owner, World};

use crate::native_world_glue::{Load, Ready, StageImages, State, decode_image, upload};

/// The world's owner of a BSP entry's geometry; `None` for an object.
pub fn owner(source: Source) -> Option<Owner> {
    match source {
        Source::Cluster(c) => Some(Owner::Cluster(u32::from(c))),
        Source::Placement(p) => Some(Owner::Instance(u32::from(p))),
        Source::Object(_) => None,
    }
}

/// Whether the parts table holds `entry`: its part, with the entry's
/// shader (by tag index).
pub fn accepts_entry(parts: &HashMap<(Owner, u8), u16>, entry: &Entry) -> bool {
    owner(entry.source).is_some_and(|o| parts.get(&(o, entry.part)) == Some(&(entry.shader as u16)))
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    fn ready(&self) -> Option<Arc<Ready>> {
        match &self.state.lock().unwrap_or_else(|e| e.into_inner()).1 {
            Load::Ready(r) => Some(Arc::clone(r)),
            Load::Idle | Load::Loading | Load::Failed => None,
        }
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn accepts(&self, _mem: &dyn GuestMemory, entry: &Entry, object: Option<&ObjectFrame>) -> bool {
        // Objects' transparent parts are not drawn here yet.
        object.is_none() && self.ready().is_some_and(|r| accepts_entry(&r.transparent_parts, entry))
    }

    fn open(&self, _camera: &RenderCamera, run: Arc<Run>) -> bool {
        let Some(ready) = self.ready() else { return false };
        let Ok(mut d) = self.device.lock() else { return false };
        if !d.can_external_pass() {
            return false;
        }
        d.request_external_pass(Box::new(RunPass {
            ready,
            run,
            view: None,
            entries: Vec::new(),
        }));
        true
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_transparents::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

/// One run's pass.
struct RunPass {
    ready: Arc<Ready>,
    run: Arc<Run>,
    /// The device's view constants at the issue.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The run's entries, taken at the issue.
    entries: Vec<(Owner, u8)>,
}

impl d3d::ExternalPass for RunPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
        self.entries = self
            .run
            .seal()
            .iter()
            .filter_map(|e| Some((owner(e.entry.source)?, e.entry.part)))
            .collect();
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let Some(constants) = self.view else { return };
        if self.entries.is_empty() {
            return;
        }
        let mut gpu = upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return };
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
        };
        gpu.encode_transparents(
            t.device,
            t.queue,
            t.encoder,
            t.color,
            t.color_format,
            t.depth,
            t.depth_format,
            t.size,
            &view,
            &self.entries,
        );
    }
}

/// At load: the transparent materials of `world` (by BSP material index;
/// `shaders` the BSP's shader per material) whose layer-15 passes all
/// translate, their stages' images added to `images`, and the parts the
/// native pass can draw with their shader's tag index. Says on stderr how
/// many and why the rest stay on the D3D path.
pub(crate) fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
) -> (HashMap<u16, Vec<TransparentProgram>>, HashMap<(Owner, u8), u16>) {
    let mut programs: HashMap<u16, Vec<TransparentProgram>> = HashMap::new();
    let mut parts = HashMap::new();
    let materials = world.transparent_materials();
    if materials.is_empty() {
        return (programs, parts);
    }
    let lib = match tags::material::ShaderLibrary::read(map) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("native-transparents: the shader tags do not read ({e}); the D3D path draws every transparent");
            return (programs, parts);
        }
    };
    let mut refused: HashMap<String, usize> = HashMap::new();
    for &m in &materials {
        let Some(&shader) = shaders.get(usize::from(m)) else { continue };
        let compiled = lib
            .material(shader, None)
            .map_err(|e| format!("material: {e}"))
            .and_then(|mat| look::transparent_passes(&mat).map_err(|r| format!("{r:?}")))
            .and_then(|passes| {
                passes
                    .iter()
                    .map(|p| {
                        let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
                            .ok_or("no world-space vertex program")?;
                        let program = program::compile_pass(&p.look, &code, p.state.alpha_test)?;
                        Ok(TransparentProgram { program, state: p.state })
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .and_then(|passes| {
                // Every stage image, or the material stays on the D3D path.
                for p in &passes {
                    for (n, s) in p.program.stages.iter().enumerate() {
                        if let Stage::Bitmap { bitmap, image } = *s
                            && p.program.dims[n].is_some()
                            && !images.contains_key(&(bitmap, image))
                        {
                            let i = decode_image(map, files, bitmap, image as usize)
                                .ok_or_else(|| format!("stage {n}'s bitmap does not decode"))?;
                            images.insert((bitmap, image), i);
                        }
                    }
                }
                Ok(passes)
            });
        match compiled {
            Ok(p) => {
                programs.insert(m, p);
            }
            Err(why) => *refused.entry(why).or_default() += 1,
        }
    }
    let mut unlit = 0;
    for t in &world.transparents {
        let Some(passes) = programs.get(&t.material) else { continue };
        let samples_lightmap = passes
            .iter()
            .any(|p| p.program.stages.iter().zip(&p.program.dims).any(|(s, d)| *s == Stage::Lightmap && d.is_some()));
        if samples_lightmap && !t.lightmap.is_some_and(|k| lightmaps.contains_key(&k)) {
            unlit += 1;
            continue;
        }
        let Some(&shader) = shaders.get(usize::from(t.material)) else { continue };
        parts.insert((t.owner, t.part), shader.0 as u16);
    }
    let mut why: Vec<_> = refused.into_iter().collect();
    why.sort();
    eprintln!(
        "native-transparents: {} of {} transparent materials translate, {} of {} parts native ({unlit} without their lightmap page); on the D3D path: {why:?}",
        programs.len(),
        materials.len(),
        parts.len(),
        world.transparents.len(),
    );
    (programs, parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(shader: u32, part: u8, source: Source) -> Entry {
        Entry {
            shader,
            part,
            source,
            depth: 0.0,
            position: [0.0; 3],
        }
    }

    #[test]
    fn entries_are_accepted_by_their_part_and_shader() {
        let parts: HashMap<(Owner, u8), u16> =
            [((Owner::Cluster(37), 2), 0x42), ((Owner::Instance(140), 0), 0x43)].into_iter().collect();
        assert!(accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Cluster(37))));
        assert!(accepts_entry(&parts, &entry(0xe123_0043, 0, Source::Placement(140))));
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 1, Source::Cluster(37))), "another part");
        assert!(!accepts_entry(&parts, &entry(0xe123_0044, 2, Source::Cluster(37))), "another shader");
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Placement(37))), "a placement is not a cluster");
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Object(0xe1a3_0004))), "objects stay on D3D");
    }
}
