//! The native fog's host (docs/graphics/native-water-fog.md, "Stage 4b"):
//! over the native world's loaded BSP (`native_world_glue`, which loads it
//! at the first layer 3 of a map), it compiles each material's layer-16
//! passes at the load, reads the fog the device holds at the layer-16
//! call, and asks the D3D device for one external pass per view
//! (`feature::native_fog`). Inert when `impl.render.native` is off
//! (nothing is compiled at load and the hook is not placed).
//!
//! - **At load** ([`load`], on the world's worker, before the world's
//!   draws are kept): each material's layer-16 passes
//!   (`native_render::fog_gpu::layer_passes`) compiled with their own
//!   programs ([`FogProgram::compile`]), all or nothing, their stage
//!   images decoded and their fog words kept; then the parts that can
//!   draw: a material that compiled, with the rigid classification for
//!   placements and its lightmap pages when a pass samples
//!   them. One line says how many and why the rest stay on the D3D path.
//! - **Per view** ([`Host::ask`], at the layer-16 call): the fog state
//!   the device holds (`d3d::nv2a::d3d_fog` over the `D3DRS_FOG*` words,
//!   read through `Device::render_state` as `stencil_state` reads the
//!   stencil, each material's own words resolved over them), and one
//!   external pass. Unreadable fog words leave the view to the D3D path.
//! - **The pass** (`FogViewPass`): the device's view constants and W clip
//!   at the issue, then `GpuWorld::encode_fog` over the loaded depth.
//!   The GPU set is uploaded by the world's `upload` ([`upload_fog`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use engine::memory::GuestMemory;
use feature::native_fog::Host;
use map::{Map, TagId};
use native_render::fog_gpu::{self, FogProgram, FogWords, FOG_WORD_SLOTS};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::program::{self, ViewInputs};
use native_render::world::{LightmapKey, Owner, World};

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, State, current_ready, palettes_hold};

/// The fog of one loaded world, kept for the whole load: what compiled
/// with its fog words, and the parts the native pass draws.
pub struct FogView {
    /// Each material's compiled layer-16 passes, in draw order (cloned
    /// into the GPU set at the upload).
    pub programs: HashMap<u16, Vec<FogProgram>>,
    /// Each material's fog words, to resolve over the device's at the ask.
    pub words: HashMap<u16, FogWords>,
    /// The parts the pass draws, in the world's draw order: owner and
    /// part, index range, lightmap page and material.
    pub parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
    /// The skip's table: each drawn part with its material's shader by
    /// tag index.
    pub part_set: HashMap<(Owner, u8), u16>,
}

impl FogView {
    /// Whether the pass draws `part` of `owner` with `shader` (the item's
    /// `+8` low word): the skip's acceptance.
    pub fn draws(&self, owner: Owner, part: u8, shader: u16) -> bool {
        self.part_set.get(&(owner, part)) == Some(&shader)
    }
}

/// Compiles every material's layer-16 passes over the world (all of its
/// materials, not only the native world's: a material that stays on the
/// D3D path for layer 3 may still fog natively) and picks the parts that
/// can draw. Called from the world's worker, before its draws are kept.
pub(crate) fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> Arc<FogView> {
    let mut programs: HashMap<u16, Vec<FogProgram>> = HashMap::new();
    let mut words: HashMap<u16, FogWords> = HashMap::new();
    let mut parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))> = Vec::new();
    let materials: Vec<u16> = world.draws.iter().map(|d| d.material).collect::<BTreeSet<_>>().into_iter().collect();
    if !materials.is_empty() {
        match tags::material::ShaderLibrary::read(map) {
            Ok(mut lib) => {
                let mut refused: HashMap<String, usize> = HashMap::new();
                let mut rigid_refused: HashMap<String, usize> = HashMap::new();
                for &m in &materials {
                    let Some(&shader) = shaders.get(usize::from(m)) else { continue };
                    let compiled = lib
                        .material(shader, None)
                        .map_err(|e| format!("material: {e}"))
                        .and_then(|mat| fog_gpu::layer_passes(&mat, native_render::fog::FOG_LAYER).map_err(|r| r.describe()))
                        .and_then(|passes| {
                            passes
                                .iter()
                                .map(|p| {
                                    let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
                                        .ok_or("no world-space vertex program")?;
                                    let mut program = FogProgram::compile(p, &code)?;
                                    // The rigid classification for the
                                    // material's placements (their rows in
                                    // c[50..52], as 0x0004d830 uploads
                                    // them). A rigid program that does not
                                    // compile leaves the placements on the
                                    // D3D path, not the whole material.
                                    if let Some(rigid) = look::rigid_program(map, lib.metadata(), p.look.vertex_shader) {
                                        match program::compile_pass_rigid(&p.look, p.state.alpha_test, &rigid) {
                                            Ok(r) => program.rigid = Some(r),
                                            Err(e) => *rigid_refused.entry(format!("rigid: {e}")).or_default() += 1,
                                        }
                                    }
                                    Ok(program)
                                })
                                .collect::<Result<Vec<_>, String>>()
                        })
                        .and_then(|passes| {
                            // Every stage image, or the material stays on
                            // the D3D path.
                            for p in &passes {
                                for (n, s) in p.program.stages.iter().enumerate() {
                                    if let Stage::Bitmap { bitmap, image } = *s
                                        && p.program.dims[n].is_some()
                                    {
                                        decoder.decode(map, files, images, bitmap, image, n, false)?;
                                    }
                                }
                            }
                            Ok(passes)
                        });
                    match compiled {
                        Ok(p) => {
                            words.insert(m, p.first().map(|p| p.words).unwrap_or_default());
                            programs.insert(m, p);
                        }
                        Err(why) => *refused.entry(why).or_default() += 1,
                    }
                }
                for d in &world.draws {
                    let Some(passes) = programs.get(&d.material) else { continue };
                    // A placement part draws only with the rigid program
                    // for every pass and its rows read
                    // ([`fog_gpu::FogSet::new`] holds the holder; without either,
                    // the part keeps its D3D draws).
                    if !passes.iter().all(|p| match d.owner {
                        Owner::Cluster(_) => p.program.inputs & !d.inputs == 0,
                        Owner::Instance(i) => world.rows(i).is_some()
                            && p.rigid.as_ref().is_some_and(|r| r.inputs & !d.inputs == 0),
                    }) {
                        continue;
                    }
                    let samples_lightmap = passes
                        .iter()
                        .any(|p| p.program.stages.iter().zip(&p.program.dims).any(|(s, dim)| *s == Stage::Lightmap && dim.is_some()));
                    if samples_lightmap && !d.lightmap.is_some_and(|k| lightmaps.contains_key(&k)) {
                        continue;
                    }
                    parts.push(((d.owner, d.part), (d.first_index, d.index_count, d.lightmap, d.material)));
                }
                let mut why: Vec<_> = refused.into_iter().collect();
                why.sort();
                let placements = world.draws.iter().filter(|d| matches!(d.owner, Owner::Instance(_))).count();
                let mut rigid_why: Vec<_> = rigid_refused.into_iter().collect();
                rigid_why.sort();
                eprintln!(
                    "native-fog: {} of {} world materials' layer-16 passes translate, {} parts native ({placements} placement parts, their rigid programs where they compiled); placements left on D3D: {rigid_why:?}; on the D3D path: {why:?}",
                    programs.len(),
                    materials.len(),
                    parts.len(),
                );
            }
            Err(e) => {
                eprintln!("native-fog: the shader tags do not read ({e}); the D3D path draws every fog pass");
            }
        }
    }
    let part_set: HashMap<(Owner, u8), u16> = parts
        .iter()
        .filter_map(|&(k, (_, _, _, m))| Some((k, shaders.get(usize::from(m))?.0 as u16)))
        .collect();
    Arc::new(FogView { programs, words, parts, part_set })
}

/// Uploads the fog set onto `gpu` (the world's `upload`, once its own and
/// the transparents' are done): `stage_image` decodes a stage's bitmap,
/// `lightmap` a page.
pub fn upload_fog(
    fog: &FogView,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu: &mut native_render::gpu::GpuWorld,
    stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
) {
    let set = fog_gpu::FogSet::new(device, queue, gpu.world(), fog.parts.clone(), fog.programs.clone(), stage_image, lightmap);
    if feature::native_verbose() {
        eprintln!("native-fog: {} materials' layer-16 passes uploaded, {} parts", set.len(), fog.parts.len());
    }
    gpu.set_fog(set);
}

/// The device's fog words at the layer-16 call, by [`FOG_WORD_SLOTS`]
/// (`None` a slot that does not read).
pub fn device_fog_words<B: d3d::backend::Backend>(d: &d3d::Device<B>, mem: &dyn GuestMemory) -> [Option<u32>; 7] {
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    FOG_WORD_SLOTS.map(|slot| table.canonical_to_guest(slot).and_then(|guest| d.render_state(&ReadMemory(mem), guest)))
}

/// The fog unit and colour of resolved words, as the D3D commit programs
/// them (`d3d::nv2a::d3d_fog`).
pub fn fog_of(words: &[u32; 7]) -> (shaders::psh::fog::Fog, [f32; 4]) {
    (
        d3d::nv2a::d3d_fog(words[0] != 0, words[1], words[2], words[3], words[4], words[5] != 0).fog,
        shaders::psh::argb(words[6]),
    )
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    state: State,
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn ask(&self, mem: &mut dyn GuestMemory) -> Option<native_render::skip::Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem)?)
        };
        let fog = ready.fog.clone()?;
        let Ok(mut d) = self.device.lock() else { return None };
        if !d.can_external_pass()
            || !d.begin_external_boundary(&mut crate::native_lights_glue::DeviceMemory(mem)).ok()?
        {
            return None;
        }
        let read = ReadMemory(mem);
        if !d.stencil_state(&read).is_some_and(|s| !s.enable)
            || !palettes_hold(&ready.palettes, &|n| d.stage_palette(&read, n))
        {
            return None;
        }
        // The fog state the device holds, each material's own words over
        // it. A slot that does not read leaves the view to the D3D path:
        // its fog cannot be established.
        let device_words = device_fog_words(&d, mem);
        let mut fogs = HashMap::new();
        for (&material, words) in &fog.words {
            let resolved = match words.resolve(&device_words) {
                Some(w) => w,
                None => {
                    if !FOG_UNREADABLE.swap(true, Ordering::Relaxed) {
                        eprintln!("native-fog: the device's fog state does not read; the D3D path draws the fog");
                    }
                    return None;
                }
            };
            fogs.insert(material, fog_of(&resolved));
        }
        // The skip over a preview: nothing touches the guest until the
        // reservation has recorded on every copy
        // (`native_render::skip::MaskPreview`).
        let planning = timeline::native::Timer::start(Stage::Fog, Metric::PrepareNs);
        let read = ReadMemory(mem);
        let mut preview = native_render::skip::MaskPreview::new(&read)?;
        let (selected, cleared) = native_render::fog::clear_fog(&mut preview, native_render::fog::FOG_LAYER as u32, &|item| {
            let accepted = fog.draws(item.owner, item.part, item.shader);
            if !accepted {
                Stage::Fog.count(Metric::Examined, 1);
                Stage::Fog.count(Metric::RefusedOwnership, 1);
            }
            accepted
        })?;
        let masks = preview.finish()?;
        let ranges = selected.len();
        if ranges == 0 {
            return None;
        }
        // What the passes write: colour always (they blend over the
        // scene), depth when any of their states writes it.
        let writes_depth = fog
            .programs
            .values()
            .any(|passes| passes.iter().any(|p| p.state.depth_write));
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let plan = d3d::ExternalPlan { stage: Stage::Fog, view, ranges, slots: Vec::new() };
        let pass = Box::new(FogViewPass {
            ready: Arc::clone(&ready),
            selected: selected.into_iter().map(|item| (item.owner, item.part)).collect(),
            fogs,
            view: None,
            w_clip: None,
            writes_depth,
            run: u64::from(d.swap_count()).wrapping_add(1),
        });
        drop(planning);
        let reservation = d
            .prepare_external_bound(&mut crate::native_lights_glue::DeviceMemory(mem), plan, pass, Vec::new())
            .ok()??;
        // Keep this load pinned until the masks and the sealed commands commit.
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, &ready)) {
            return None;
        }
        if !masks.apply(&mut feature::native_fog::Mem(mem)) {
            Stage::Fog.count(Metric::RefusedMasks, ranges as u64);
            return None;
        }
        reservation.commit();
        Some(cleared)
    }
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_fog::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

static FOG_UNREADABLE: AtomicBool = AtomicBool::new(false);

/// One view's fog replacement.
struct FogViewPass {
    ready: Arc<Ready>,
    /// The parts the masks surrendered, in the layer's item order: what
    /// the replacement must record on every copy.
    selected: Vec<(Owner, u8)>,
    /// Each material's fog unit and colour, resolved at the ask.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    w_clip: Option<([f32; 2], bool)>,
    /// Whether any of the passes' states writes depth (the write
    /// declaration).
    writes_depth: bool,
    /// The swap the view is in: the run's key, so attachments are pinned
    /// once (`d3d::ExternalPass::batch_key`).
    run: u64,
}

impl d3d::ExternalPass for FogViewPass {
    /// Packets need no worker round trip, and the pass samples no guest
    /// texture, level or slot (its stage textures are its own).
    fn batch_key(&self) -> Option<u64> {
        Some(self.run)
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.w_clip = (clip.w_buffered == Some(true)).then_some((clip.range, clip.clamp));
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.view.is_none() || self.w_clip.is_none() {
            return Err("fog view constants or W-buffer clip unavailable");
        }
        let mut guard = crate::native_world_glue::upload(&self.ready, t);
        let gpu = guard.as_mut().ok_or("world geometry unavailable")?;
        let set = gpu.fog().ok_or("fog materials unavailable")?;
        for &(owner, part) in &self.selected {
            if !set.draws_part(owner, part) {
                return Err("a selected fog part did not bind");
            }
        }
        Ok(())
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: self.writes_depth }
    }

    /// The fog's draws join the D3D render pass already open on these
    /// attachments, their uniforms written ahead of them
    /// (`ExternalDraws::writes`): no pass or command buffer of their own.
    /// A refusal here leaves the D3D draws.
    fn record_draws(
        &mut self, t: &d3d::backend::ExternalAttachments, request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalDraws, &'static str>> {
        let Some(constants) = self.view else { return Some(Err("fog view constants unavailable")) };
        if self.w_clip.is_none() {
            return Some(Err("fog W-buffer clip unavailable"));
        }
        let mut guard = crate::native_world_glue::upload_on(&self.ready, &t.device, &t.queue);
        let Some(gpu) = guard.as_mut() else { return Some(Err("world geometry unavailable")) };
        let view = ViewInputs {
            constants,
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: self.w_clip,
        };
        Some(gpu.fog_draws(&t.device, t.color_format, t.depth_format, t.size, &self.selected, &view, &self.fogs))
    }

    fn record_ranges(&mut self, mut t: d3d::ExternalTarget<'_>) -> usize {
        let Some(constants) = self.view else {
            if !NO_VIEW.swap(true, Ordering::Relaxed) {
                eprintln!("native-fog: a replacement came without the device's view constants; it records nothing");
            }
            return 0;
        };
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: self.w_clip,
        };
        let mut gpu = crate::native_world_glue::upload(&self.ready, &t);
        let Some(gpu) = gpu.as_mut() else { return 0 };
        {
            let mut uploads = t.encoder_uploads();
            gpu.prepare_fog(&mut uploads, &view, &self.fogs);
        }
        gpu.encode_fog(t.device, t.encoder, t.color, t.color_format, t.depth, t.depth_format, t.size, &self.selected)
    }
}

static NO_VIEW: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fog_words_overlay_the_device() {
        let own = FogWords { enable: Some(1), ..Default::default() };
        let device = [Some(0), Some(3), Some(1.0f32.to_bits()), Some(9.0f32.to_bits()), Some(0), Some(0), Some(0x00ff_ff80)];
        let w = own.resolve(&device).unwrap();
        assert_eq!(w[0], 1, "the pass's own FOGENABLE wins");
        assert_eq!(
            w[1..].to_vec(),
            device[1..].iter().map(|w| w.unwrap()).collect::<Vec<_>>(),
            "the rest is the device's"
        );
        let none = [None; 7];
        assert_eq!(own.resolve(&none), None, "a slot nothing sets does not resolve");
        assert_eq!(FogWords::default().resolve(&device), Some(device.map(|w| w.unwrap())));
    }

    #[test]
    fn the_fog_unit_is_d3d_s() {
        // `d3d_fog` is the authority; a smoke check that the words reach
        // it as it expects: fog off, and linear with start/end.
        let off = fog_of(&[0, 3, 0, 0, 0, 0, 0]);
        assert_eq!(off.0, shaders::psh::fog::Fog::OFF);
        let linear = fog_of(&[1, 3, 2.0f32.to_bits(), 10.0f32.to_bits(), 0, 0, 0xff00_0080]);
        let f = linear.0;
        assert!(f.enable);
        // `(end·k + 1, −k)` for `k = 1/(end−start)`; 4e-6 relative is
        // `d3d_fog`'s own tolerance.
        assert!((f.bias - 10.0 / 8.0 - 1.0).abs() < 1e-6);
        assert!((f.scale + 1.0 / 8.0).abs() < 1e-6);
        assert_eq!(linear.1, shaders::psh::argb(0xff00_0080));
    }
}
