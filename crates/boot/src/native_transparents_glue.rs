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
//! - **Per entry** ([`Host::accepts`]): a BSP cluster's or instanced
//!   geometry's part in that table whose shader is the entry's; an
//!   object's (with the native objects on, which copy its nodes) when its
//!   definition's model is loaded with that node count and takes that
//!   shader's transparent materials (`native_objects_glue::Models`).
//! - **Per run** ([`Host::open`]): one external pass. The device issues it
//!   at its next entry; it takes the run's entries then
//!   (`ExternalPass::view_constants`, which seals the run) and draws them
//!   in order, consecutive world entries with `GpuWorld::encode_transparents`
//!   and consecutive objects with `GpuModels::encode_transparents`.

use std::collections::HashMap;
use std::sync::Arc;

use engine::memory::GuestMemory;
use feature::native_transparents::Host;
use map::{Map, TagId};
use native_render::camera::RenderCamera;
use native_render::gpu::{DepthRule, Image, PassViewport};
use native_render::look::{self, Stage};
use native_render::model::NodeMatrix;
use native_render::model_gpu::TransparentItem;
use native_render::objects::ObjectFrame;
use native_render::program::{self, ViewInputs};
use native_render::transparent::{Entry, Run, RunEntry, Source};
use native_render::transparent_gpu::TransparentProgram;
use native_render::world::{LightmapKey, Owner, World};

use crate::native_objects_glue::{Models, current_models, upload_pending};
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
        match object {
            Some(o) => current_models().is_some_and(|m| m.transparent_materials(o, entry.shader).is_some()),
            None => self.ready().is_some_and(|r| accepts_entry(&r.transparent_parts, entry)),
        }
    }

    fn open(&self, camera: &RenderCamera, run: Arc<Run>) -> bool {
        let (ready, models) = (self.ready(), current_models());
        if ready.is_none() && models.is_none() {
            return false;
        }
        let Ok(mut d) = self.device.lock() else { return false };
        if !d.can_external_pass() {
            return false;
        }
        d.request_external_pass(Box::new(RunPass {
            ready,
            models,
            camera: *camera,
            run,
            view: None,
            segments: Vec::new(),
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

/// An object's transparent parts in a run: the materials its entry's
/// shader draws, posed by the nodes the hook copied.
#[derive(Clone, Debug, PartialEq)]
struct ObjectItem {
    definition: u16,
    materials: Vec<u16>,
    nodes: Vec<NodeMatrix>,
}

/// Consecutive entries of one kind, drawn by one pass of the world's or
/// the models'.
#[derive(Clone, Debug, PartialEq)]
enum Segment {
    World(Vec<(Owner, u8)>),
    Objects(Vec<ObjectItem>),
}

/// A sealed run in order as segments; `materials` names an object entry's
/// materials (`None`: not drawn).
fn segments(entries: Vec<RunEntry>, materials: &dyn Fn(&ObjectFrame, u32) -> Option<Vec<u16>>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for e in entries {
        match (owner(e.entry.source), e.object) {
            (Some(o), _) => match out.last_mut() {
                Some(Segment::World(w)) => w.push((o, e.entry.part)),
                _ => out.push(Segment::World(vec![(o, e.entry.part)])),
            },
            (None, Some(frame)) => {
                let Some(materials) = materials(&frame, e.entry.shader) else { continue };
                let item = ObjectItem {
                    definition: frame.definition as u16,
                    materials,
                    nodes: frame.nodes,
                };
                match out.last_mut() {
                    Some(Segment::Objects(v)) => v.push(item),
                    _ => out.push(Segment::Objects(vec![item])),
                }
            }
            (None, None) => {}
        }
    }
    out
}

/// One run's pass.
struct RunPass {
    ready: Option<Arc<Ready>>,
    models: Option<Arc<Models>>,
    camera: RenderCamera,
    run: Arc<Run>,
    /// The device's view constants at the issue.
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    /// The run's entries, taken at the issue.
    segments: Vec<Segment>,
}

impl d3d::ExternalPass for RunPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
        let models = self.models.clone();
        self.segments = segments(self.run.seal(), &|o, shader| models.as_ref()?.transparent_materials(o, shader));
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        let Some(constants) = self.view else { return };
        let [x, y, width, height] = t.viewport;
        for segment in &self.segments {
            match segment {
                Segment::World(entries) => {
                    let Some(ready) = &self.ready else { continue };
                    let mut gpu = upload(ready, &t);
                    let Some(gpu) = gpu.as_mut() else { continue };
                    let view = ViewInputs {
                        constants,
                        guest_size: t.guest_size,
                        size: t.size,
                        z_scale: t.z_scale,
                    };
                    gpu.encode_transparents(
                        t.device,
                        t.queue,
                        &mut *t.encoder,
                        t.color,
                        t.color_format,
                        t.depth,
                        t.depth_format,
                        t.size,
                        &view,
                        entries,
                    );
                }
                Segment::Objects(objects) => {
                    let Some(models) = &self.models else { continue };
                    let mut gpu = upload_pending(models, &t);
                    let Some(gpu) = gpu.as_mut() else { continue };
                    let index = models.index.lock().unwrap_or_else(|e| e.into_inner());
                    let items: Vec<TransparentItem<'_>> = objects
                        .iter()
                        .filter_map(|o| Some((*index.get(&o.definition)?, o)))
                        .flat_map(|(model, o)| {
                            o.materials.iter().map(move |&material| TransparentItem {
                                model,
                                material,
                                nodes: &o.nodes,
                            })
                        })
                        .collect();
                    drop(index);
                    gpu.encode_transparents(
                        t.device,
                        &mut *t.encoder,
                        t.color,
                        t.color_format,
                        t.depth,
                        t.depth_format,
                        PassViewport { x, y, width, height },
                        &self.camera,
                        DepthRule {
                            full_scale: t.z_scale,
                            floor: true,
                        },
                        &items,
                    );
                }
            }
        }
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
    let mut lib = match tags::material::ShaderLibrary::read(map) {
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

    fn frame(definition: u32, nodes: usize) -> ObjectFrame {
        ObjectFrame {
            handle: 0x8001_0001,
            definition,
            nodes: vec![NodeMatrix::IDENTITY; nodes],
            sphere: ([0.0; 3], 1.0),
        }
    }

    #[test]
    fn a_run_splits_into_world_and_object_segments_in_its_order() {
        let world = |part| RunEntry {
            entry: entry(0xe123_0042, part, Source::Cluster(37)),
            object: None,
        };
        let object = |shader, definition| RunEntry {
            entry: entry(shader, 0, Source::Object(0x8001_0001)),
            object: Some(frame(definition, 2)),
        };
        let materials = |o: &ObjectFrame, shader: u32| (shader as u16 == 0x50).then(|| vec![o.definition as u16 & 0xf]);
        let run = vec![
            world(0),
            world(1),
            object(0xe123_0050, 0xe000_0101),
            object(0xe123_0051, 0xe000_0102),
            object(0xe123_0050, 0xe000_0103),
            world(2),
            RunEntry {
                entry: entry(0xe123_0050, 0, Source::Object(0x8001_0001)),
                object: None,
            },
        ];
        let item = |definition: u16| ObjectItem {
            definition,
            materials: vec![definition & 0xf],
            nodes: vec![NodeMatrix::IDENTITY; 2],
        };
        assert_eq!(
            segments(run, &materials),
            vec![
                Segment::World(vec![(Owner::Cluster(37), 0), (Owner::Cluster(37), 1)]),
                Segment::Objects(vec![item(0x0101), item(0x0103)]),
                Segment::World(vec![(Owner::Cluster(37), 2)]),
            ],
            "a shader the model does not take and an object without its copy are not drawn"
        );
    }
}
