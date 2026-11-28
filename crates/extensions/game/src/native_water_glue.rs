//! The native water's host (docs/graphics/native-water-fog.md, "Stage 4b:
//! water and the rest"): over the native world's loaded BSP, it compiles
//! the transparent parts' layer-6 passes at the load ([`load`]), and at
//! the layer-6 call puts the view's water in the frame step's scene
//! (`frame_scene::WaterState`) and pushes a stage that draws the scene's
//! copy (`render_bridge::native_water`, `native_fog_glue::commit_late`).
//!
//! A sampled render target is the frame graph's surface for its role
//! (`TargetRef::Game`, [`sampled_targets`]), which the stage names
//! (`native_render::Stage::samples`) and is handed in place. The resolver
//! of its texture header at `+4` in the target descriptor (`0x004b4b58`,
//! stride `0x98`), `0x0001dcc0`, requires a live surface at `+0x88` and a
//! clear byte at `+0x95` ([`target_header`]). Engine texture 5 samples a
//! copy of the bound colour for the view's own target.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use guest_layout::memory::GuestMemory;
use engine::screen_effect::{TARGET_STRIDE, TARGETS};
use render_bridge::native_water::{Host, PartKey};
use frame_scene::transparent::DisplayTransparentPart;
use map::{Map, TagId};
use native_render::fog_gpu::{FogProgram, FogWords};
use native_render::look::{self, Stage};
use native_render::program::ViewInputs;
use native_render::water_gpu::{self, Supply, WaterSet, WATER_LAYER};
use native_render::{Stage as FrameStage, StageWork};
use frame_types::{Slot, TargetRef, TargetRole};
use native_render::world::{LightmapKey, Owner, World};
use frame_scene::LatePart;
use render_bridge::late_sorted::{Run, RunAt};

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, State, current_ready};

/// The water of one loaded world: what compiled, the parts the native
/// pass draws, and the engine-texture supplies it needs per view.
pub struct WaterData {
    /// Shader datums whose pool surface draws from the sorted layer-15 list.
    pub pools: Arc<BTreeSet<u32>>,
    /// Each material's resolved layer-6 passes (with the engine stages
    /// kept) and their compiled programs, in draw order.
    pub passes: HashMap<u16, Vec<native_render::fog_gpu::FogPass>>,
    pub programs: HashMap<u16, Vec<FogProgram>>,
    /// The fog words each material's passes set themselves (a water pass
    /// that reads the fog register gets the device's fog).
    pub words: HashMap<u16, FogWords>,
    /// The parts the pass draws, in the transparents' order: owner and
    /// part, index range, lightmap page and material.
    pub parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))>,
    /// The skip's table: each part with its material's shader by tag
    /// index.
    pub part_set: HashMap<(Owner, u8), u16>,
    /// The distinct render targets the passes sample (their engine
    /// stages' `Target` supplies), ascending.
    pub targets: Vec<u32>,
    /// Whether any pass samples the view's own target.
    pub samples_view: bool,
    /// Each edge material's shader (tag index) with the implementation
    /// each of its layer-6 passes binds: what skipping its guest draw is
    /// checked against when a kept entry follows ([`WaterData::skip_holds`]).
    pub implementations: HashMap<u16, Vec<tags::material::ResolvedImplementation>>,
}

impl WaterData {
    /// As `native_fog_glue::FogView::skip_holds`, for layer 6.
    pub fn skip_holds(&self, shader: u16, read: &dyn Fn(u32) -> Option<u32>) -> bool {
        self.implementations
            .get(&shader)
            .is_some_and(|imps| imps.iter().all(|imp| native_render::layer_pass::skip_holds(imp, read)))
    }
}

/// The embedded texture header returned by `0x0001dcc0` for a live target.
pub(crate) fn target_header(mem: &dyn GuestMemory, target: u32) -> Option<u32> {
    let descriptor = TARGETS.checked_add(TARGET_STRIDE.checked_mul(target)?)?;
    let disabled = descriptor.checked_add(0x95)?;
    if mem.u32(descriptor + 0x88).ok()? == 0 || mem.u8(disabled).ok()? != 0 {
        return None;
    }
    Some(descriptor + 4)
}

/// Loads the water edge blend's passes over the world's transparent
/// parts (the water surfaces), called from the world's worker before its
/// draws are kept. One line says how many translated and why the rest
/// stay on the D3D path.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, native_render::gpu::Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> WaterData {
    let mut passes: HashMap<u16, Vec<native_render::fog_gpu::FogPass>> = HashMap::new();
    let mut programs: HashMap<u16, Vec<FogProgram>> = HashMap::new();
    let mut words: HashMap<u16, FogWords> = HashMap::new();
    let mut parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))> = Vec::new();
    let materials: Vec<u16> = world.transparents.iter().map(|t| t.material).collect::<BTreeSet<_>>().into_iter().collect();
    let mut pools = BTreeSet::new();
    let mut implementations = HashMap::new();
    if !materials.is_empty() {
        if let Ok(mut lib) = tags::material::ShaderLibrary::read(map) {
            let rasterizer = map.tags().iter().find(|tag| tag.group.0 == *b"matg")
                .and_then(|tag| tag.location)
                .and_then(|loc| crate::native_lights_glue::matg_rasterizer_bitmap(
                    lib.metadata(), loc.address, crate::native_lights_glue::RASTERIZER_DATUM_14,
                ));
            let mut refused: HashMap<String, usize> = HashMap::new();
            let mut edge_materials = 0;
            for &m in &materials {
                let Some(&shader) = shaders.get(usize::from(m)) else {
                    *refused.entry("missing BSP material shader".into()).or_default() += 1;
                    continue;
                };
                // The sorted handler reads the stem from the compiled instance.
                // Its live pass supplies animated images and constants at issue.
                let routes = lib.shader(shader).and_then(|instance| {
                    let id = instance.compiled()?.template;
                    let name = map.tag(id).ok().and_then(|t| t.name.as_deref());
                    let template = lib.template(id)?.compiled()?;
                    Ok((water_gpu::sorted_pool(name, template),
                        template.layer_passes(0, WATER_LAYER).is_some_and(|p| !p.is_empty())))
                });
                let edge = match routes {
                    Ok((sorted, edge)) => {
                        if sorted { pools.insert(shader.0); }
                        edge
                    }
                    Err(e) => {
                        *refused.entry(format!("template lookup: {e}")).or_default() += 1;
                        continue;
                    }
                };
                if !edge { continue; }
                edge_materials += 1;
                let compiled = lib.material(shader, None).map_err(|e| format!("material: {e}"))
                    .and_then(|mat| {
                        // A water pass has one implementation (`supplied_layer_passes`).
                        if let Some(imps) = native_render::fog_gpu::layer_implementations(&mat, WATER_LAYER, None) {
                            implementations.insert(shader.0 as u16, imps);
                        }
                        water_gpu::water_passes(&mat, rasterizer).map_err(|r| r.describe())
                    })
                    .and_then(|ps| {
                        ps.iter()
                            .map(|p| {
                                let code = look::world_program(map, lib.metadata(), p.look.vertex_shader)
                                    .ok_or("no world-space vertex program")?;
                                let mut program = FogProgram::compile(p, &code)?;
                                if let Some(code) = look::rigid_program(map, lib.metadata(), p.look.vertex_shader) {
                                    program.rigid = native_render::program::compile_pass_rigid(&p.look, p.state.alpha_test, &code).ok();
                                }
                                Ok::<_, String>(program)
                            })
                            .collect::<Result<Vec<_>, String>>()
                            .map(|compiled| (ps, compiled))
                    })
                    .and_then(|(ps, compiled)| {
                        // Every sampled stage's image, or the material
                        // stays on the D3D path (the compiled program's
                        // dims name what it samples).
                        for (p, c) in ps.iter().zip(&compiled) {
                            for (n, s) in p.look.textures.iter().enumerate() {
                                if let Stage::Bitmap { bitmap, image } = *s
                                    && c.program.dims[n].is_some()
                                {
                                    decoder.decode(map, files, images, bitmap, image, n, false)?;
                                }
                            }
                        }
                        Ok((ps, compiled))
                    });
                match compiled {
                    Ok((ps, compiled)) => {
                        words.insert(m, compiled.first().map(|p| p.words).unwrap_or_default());
                        passes.insert(m, ps);
                        programs.insert(m, compiled);
                    }
                    Err(why) => *refused.entry(why).or_default() += 1,
                }
            }
            let mut unlit = 0;
            for t in &world.transparents {
                let Some(compiled) = programs.get(&t.material) else { continue };
                if let Owner::Instance(i) = t.owner {
                    let refusal = if world.rows(i).is_none() {
                        Some("placement_rows")
                    } else if compiled.iter().any(|p| p.rigid.is_none()) {
                        Some("rigid_program")
                    } else if compiled.iter().any(|p| p.rigid.as_ref().is_some_and(|r| r.inputs & !t.inputs != 0)) {
                        Some("placement_streams")
                    } else {
                        None
                    };
                    if let Some(cause) = refusal {
                        *refused.entry(cause.into()).or_default() += 1;
                        render_bridge::native_layers::count(WATER_LATE_LAYER.into(), &format!("refused_{cause}_load"), 1);
                        continue;
                    }
                }
                let samples_lightmap = compiled.iter().any(|c| {
                    c.program.stages.iter().zip(&c.program.dims).any(|(s, d)| matches!(s, Stage::Lightmap) && d.is_some())
                });
                if samples_lightmap && !t.lightmap.is_some_and(|k| lightmaps.contains_key(&k)) {
                    unlit += 1;
                    continue;
                }
                parts.push(((t.owner, t.part), (t.first_index, t.index_count, t.lightmap, t.material)));
            }
            let mut why: Vec<_> = refused.into_iter().collect();
            why.sort();
            eprintln!(
                "native-water: {} of {edge_materials} layer-6 edge materials translate, {} edge parts eligible ({unlit} without lightmap); {} sorted pool shaders use live layer 15; refusals: {why:?}",
                programs.len(),
                parts.len(),
                pools.len(),
            );
        } else {
            eprintln!("native-water: shader library unreadable for {} transparent materials", materials.len());
        }
    }
    let mut targets: Vec<u32> = passes
        .values()
        .flat_map(|ps| {
            ps.iter()
                .flat_map(|p| p.look.textures.iter().filter_map(|s| water_gpu::supply_of(*s)))
                .filter_map(|s| match s { Supply::Target(t) => Some(t), Supply::View => None })
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    let samples_view = passes
        .values()
        .any(|ps| ps.iter().any(|p| p.look.textures.iter().any(|s| water_gpu::supply_of(*s) == Some(Supply::View))));
    let part_set: HashMap<(Owner, u8), u16> = parts
        .iter()
        .filter_map(|&(k, (_, _, _, m))| Some((k, shaders.get(usize::from(m))?.0 as u16)))
        .collect();
    WaterData {
        pools: Arc::new(pools),
        passes,
        programs,
        words,
        parts,
        part_set,
        targets,
        samples_view,
        implementations,
    }
}

/// Builds the water resources on the BSP loader's device.
pub fn upload_water(
    water: &WaterData,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &World,
    stage_image: &mut dyn FnMut(Stage) -> Option<native_render::gpu::Image>,
    lightmap_image: &mut dyn FnMut(LightmapKey) -> Option<native_render::gpu::Image>,
) -> WaterSet {
    let mut programs = HashMap::new();
    for (&m, compiled) in &water.programs {
        if let Some(ps) = water.passes.get(&m) {
            programs.insert(m, (ps.clone(), compiled.clone()));
        }
    }
    let set = WaterSet::new(
        device, queue, world, water.parts.clone(), programs, water.targets.clone(), water.samples_view,
        stage_image, lightmap_image,
    );
    if hooks::native_verbose() {
        eprintln!(
            "native-water: {} materials' layer-6 passes uploaded, {} parts, targets {:?}, samples the view: {}",
            set.len(),
            water.parts.len(),
            water.targets,
            water.samples_view
        );
    }
    set
}

struct Glue {
    state: State,
    /// The parts the last `ask` drew at layer 6 for the display's wider
    /// view, until the view's display runs take them.
    layer6_display: std::sync::Mutex<Vec<PartKey>>,
}

/// A queue part as the water set names it.
fn part_owner(&(cluster, part): &PartKey) -> (Owner, u8) {
    (Owner::Cluster(u32::from(cluster)), part)
}

/// The frame targets of the render targets the passes sample, in
/// `WaterData::targets`' order: each live ([`target_header`]) and named by
/// its role's first surface, else `target_view`.
fn sampled_targets(mem: &dyn GuestMemory, water: &WaterData) -> Result<Vec<TargetRef>, &'static str> {
    water.targets.iter().map(|&t| frame_target(mem, t)).collect()
}

/// Render target `t`'s frame target, when it is live.
fn frame_target(mem: &dyn GuestMemory, t: u32) -> Result<TargetRef, &'static str> {
    target_header(mem, t).ok_or("target_view")?;
    let role = TargetRole::Game(u8::try_from(t).map_err(|_| "target_view")?);
    let header = render_bridge::native_targets::surface(role).ok_or("target_view")?;
    match crate::native_targets_glue::target_ref(mem, header, true) {
        r @ TargetRef::Game(_) => Ok(r),
        _ => Err("target_view"),
    }
}

impl Glue {
    /// Pushes `parts` (from inside the sorted dispatch, parts only the
    /// display's wider view sees) as one stage. `None` leaves them undrawn.
    fn draw_display(&self, mem: &mut dyn GuestMemory, parts: &[PartKey]) -> Option<usize> {
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, mem)?)
        };
        let (water, _) = ready.water.get()?;
        let selected: Vec<(Owner, u8)> = parts.iter().map(part_owner).collect();
        if selected.is_empty() || selected.iter().any(|key| !water.part_set.contains_key(key)) {
            return None;
        }
        let targets = sampled_targets(mem, water).ok()?;
        let device_words = crate::native_fog_glue::device_fog_words(mem);
        let Some(materials) = crate::native_fog_glue::resolve_materials(&water.words, &device_words) else {
            unimplemented!("water: a fog word a material leaves to the render-state shadow does not read");
        };
        let count = selected.len();
        let pass = WaterViewPass {
            ready: Arc::clone(&ready),
            selected,
            fogs: crate::native_fog_glue::late_fogs(&materials),
            samples_view: water.samples_view,
            targets,
        };
        crate::native_targets_glue::push(&*mem, Slot::AfterLights, pass);
        Some(count)
    }
}

impl Host for Glue {
    fn draws_display_part(&self, mem: &dyn GuestMemory, part: &DisplayTransparentPart) -> bool {
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match current_ready(&state, mem) {
                Some(ready) => Arc::clone(ready),
                None => return false,
            }
        };
        let Some((water, _)) = ready.water.get() else { return false };
        // The part's live material is the one the set compiled.
        water.part_set.get(&part_owner(&(part.cluster, part.part))) == Some(&((part.shader & 0xffff) as u16))
    }

    fn draw_display_parts(&self, mem: &mut dyn GuestMemory, parts: &[PartKey]) -> usize {
        self.draw_display(mem, parts).unwrap_or(0)
    }

    fn take_layer6_display(&self) -> Vec<PartKey> {
        std::mem::take(&mut *self.layer6_display.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn ask(&self, mem: &mut dyn GuestMemory, display_clusters: &[u16], left: &mut render_bridge::native_layers::Left) -> Option<native_render::skip::Cleared> {
        self.ask_at(mem, display_clusters, left, None, &mut Run::Refused)
    }

    fn ask_run(&self, mem: &mut dyn GuestMemory, at: &RunAt<'_>, left: &mut render_bridge::native_layers::Left) -> Run {
        let mut run = Run::Refused;
        self.ask_at(mem, &[], left, Some(at), &mut run);
        run
    }
}

impl Glue {
    /// As the fog's (`native_fog_glue`): before the build, or for one run
    /// of the sorted entries, whose outcome goes to `out`. The display
    /// part stays the ask's before the build.
    fn ask_at(
        &self, mem: &mut dyn GuestMemory, display_clusters: &[u16], left: &mut render_bridge::native_layers::Left,
        at: Option<&RunAt<'_>>, out: &mut Run,
    ) -> Option<native_render::skip::Cleared> {
        use timeline::native::{Metric, Stage};
        if at.is_none() {
            self.layer6_display.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(ready) = current_ready(&state, mem) else { return crate::native_fog_glue::kept(left, "unloaded") };
            Arc::clone(ready)
        };
        let Some((water, _)) = ready.water.get() else { return crate::native_fog_glue::kept(left, "unloaded") };
        if water.parts.is_empty() {
            return crate::native_fog_glue::kept(left, "no_parts");
        }
        let targets = match sampled_targets(mem, water) {
            Ok(targets) => targets,
            Err(cause) => return crate::native_fog_glue::refuse_view(left, WATER_LATE_LAYER, cause),
        };
        // The fog state, as the fog's.
        let device_words = crate::native_fog_glue::device_fog_words(mem);
        let Some(materials) = crate::native_fog_glue::resolve_materials(&water.words, &device_words) else {
            unimplemented!("water: a fog word a material leaves to the render-state shadow does not read");
        };
        let pass = |selected, fogs| WaterViewPass {
            ready: Arc::clone(&ready),
            selected,
            fogs,
            samples_view: water.samples_view,
            targets: targets.clone(),
        };
        if !display_clusters.is_empty() {
            let selected = crate::native_fog_glue::display_late_part(mem, display_clusters, &water.part_set, WATER_LATE_LAYER, |_, _| true);
            if let Some(selected) = selected {
                crate::native_targets_glue::push(&*mem, Slot::AfterLights, pass(vec![selected], crate::native_fog_glue::late_fogs(&materials)));
                if let Owner::Cluster(cluster) = selected.0
                    && let Ok(cluster) = u16::try_from(cluster)
                {
                    *self.layer6_display.lock().unwrap_or_else(|e| e.into_inner()) = vec![(cluster, selected.1)];
                }
            }
        }
        let read = ReadMemory(mem);
        let Some(mut preview) = native_render::skip::MaskPreview::new(&read) else { return crate::native_fog_glue::kept(left, "list") };
        let (selected, cleared, run) = match at {
            None => {
                let Some((selected, cleared, refused)) = native_render::fog::clear_fog(&mut preview, WATER_LAYER as u32, &|item| {
                    let accepted = water.part_set.get(&(item.owner, item.part)) == Some(&(item.shader));
                    if !accepted {
                        Stage::Water.count(Metric::Examined, 1);
                        Stage::Water.count(Metric::RefusedOwnership, 1);
                    }
                    accepted
                }) else { return crate::native_fog_glue::kept(left, "list") };
                crate::native_fog_glue::count_late_refusals(WATER_LAYER as u32, refused, left);
                (selected, cleared, None)
            }
            Some(at) => {
                let Some(restore) = native_render::sorted_entries::cached_restore(&read) else {
                    return crate::native_fog_glue::kept(left, "cached_state");
                };
                let state = native_render::sorted_entries::after_restore(&restore, &|_| None);
                let Some(run) = native_render::fog::sorted_run(&read, WATER_LAYER as u32, &at.sorted, at.from, &|item| {
                    if water.part_set.get(&(item.owner, item.part)) != Some(&item.shader) {
                        Err("material")
                    } else if !water.skip_holds(item.shader, &state) {
                        Err("state")
                    } else {
                        Ok(())
                    }
                }) else { return crate::native_fog_glue::kept(left, "list") };
                crate::native_fog_glue::run_stop(WATER_LAYER as u32, run.stop, left);
                if run.drawn.is_empty() {
                    *out = Run::Stopped;
                    return None;
                }
                let cleared = native_render::skip::Cleared { layer: WATER_LAYER as u32, items: Vec::new() };
                (run.drawn.clone(), cleared, Some((at, run)))
            }
        };
        let Some(masks) = preview.finish() else { return crate::native_fog_glue::kept(left, "list") };
        if selected.is_empty() {
            return None;
        }
        // The view's water into the frame step's scene, the layer's
        // earlier runs first; the stage draws the scene's copy of this run.
        let prior = run.as_ref().map_or(&[][..], |(at, _)| at.prior);
        let mut parts: Vec<LatePart> = prior.to_vec();
        parts.extend(selected.iter().map(|item| LatePart { owner: item.owner, part: item.part }));
        let Some(late) = render_bridge::scene_step::late_layer(&*mem, WATER_LATE_LAYER, true, device_words, &materials, &parts) else {
            return crate::native_fog_glue::kept(left, "scene");
        };
        let fogs = crate::native_fog_glue::late_fogs(&late.materials);
        let Some(this_run) = late.parts.get(prior.len()..) else { return crate::native_fog_glue::kept(left, "scene") };
        let selected = crate::native_fog_glue::late_selected(this_run);
        if !crate::native_fog_glue::commit_late(mem, &self.state, &ready, pass(selected, fogs), &masks) {
            return crate::native_fog_glue::kept(left, "reservation");
        }
        render_bridge::scene_step::late_drawn(late.key, WATER_LATE_LAYER);
        if let Some((_, run)) = run {
            *out = Run::Drawn { slots: run.slots, entries: run.entries, parts: this_run.to_vec() };
        }
        Some(cleared)
    }
}

/// The water's layer, as the scene names it.
const WATER_LATE_LAYER: u8 = WATER_LAYER as u8;

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install(state: State) {
    render_bridge::native_water::set_host(Arc::new(Glue { state, layer6_display: Default::default() }));
}

/// One view's water.
struct WaterViewPass {
    ready: Arc<Ready>,
    /// The parts the masks surrendered, in the layer's item order.
    selected: Vec<(Owner, u8)>,
    /// Each material's fog unit and colour, resolved at the ask.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
    samples_view: bool,
    /// The render targets the passes sample, in `WaterData::targets`'
    /// order.
    targets: Vec<TargetRef>,
}

impl WaterViewPass {
    /// What the bound colour must be when a material samples the view: one
    /// that can be copied and filtered.
    fn preflight(&self, t: &gfx::external::ExternalAttachments) -> Result<(), &'static str> {
        if self.samples_view {
            if !gfx::program::filterable(t.color_format, t.device.features()) {
                return Err("the bound colour is not filterable for the scene stages");
            }
            if t.color.texture().format() != t.color_format || !WaterSet::can_copy(&t.color, t.size) {
                return Err("the bound colour cannot be copied for the scene stages");
            }
        }
        Ok(())
    }
}

impl FrameStage for WaterViewPass {
    fn name(&self) -> &'static str {
        "water"
    }

    fn samples(&self) -> Vec<TargetRef> {
        self.targets.clone()
    }

    /// The water's draws join the render pass open on the attachments; a
    /// set that samples the view's own target copies it first, so it
    /// records encoded commands.
    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> StageWork {
        if view.w_clip.is_none() {
            unimplemented!("water: the view's W-buffer clip");
        }
        if let Err(why) = self.preflight(t) {
            unimplemented!("water: {why}");
        }
        let viewport = [0.0, 0.0, t.size.0 as f32, t.size.1 as f32];
        let sampled: Vec<Option<wgpu::TextureView>> = t.sampled.iter().map(|l| l.as_ref().map(|l| l.view.clone())).collect();
        let mut guard = self.ready.gpu();
        let Some(gpu) = guard.as_mut() else { unimplemented!("water: world geometry unavailable") };
        let work = if self.samples_view {
            gpu.water_encoded(
                &t.device, &t.queue, &t.uploads, &t.color, t.color_format, &t.depth, t.depth_format,
                t.size, viewport, gfx::external::TargetCopy::Display, &self.selected, &sampled, view, &self.fogs,
            )
            .map(StageWork::Encoded)
        } else {
            gpu.water_draws(&t.device, t.color_format, t.depth_format, t.size, viewport, &self.selected, &sampled, view, &self.fogs)
                .map(StageWork::Draws)
        };
        work.unwrap_or_else(|why| unimplemented!("water: {why}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn target_header_uses_the_embedded_texture_of_a_live_target() {
        let mut mem = FlatMemory::new(TARGETS, (2 * TARGET_STRIDE) as usize);
        let descriptor = TARGETS + TARGET_STRIDE;
        assert_eq!(target_header(&mem, 1), None);

        mem.set_u32(descriptor + 0x88, 0x1000).unwrap();
        mem.set_u8(descriptor + 0x94, 1).unwrap();
        assert_eq!(target_header(&mem, 1), Some(native_render::object_nodes::TEXACCUM_HEADER));

        mem.set_u8(descriptor + 0x95, 1).unwrap();
        assert_eq!(target_header(&mem, 1), None);
        assert_eq!(target_header(&mem, 2), None);
    }

    #[test]
    fn a_sampled_target_is_its_roles_frame_target() {
        let mut mem = FlatMemory::new(TARGETS, (0x20 * TARGET_STRIDE) as usize);
        let descriptor = TARGETS + 0x16 * TARGET_STRIDE;
        assert_eq!(frame_target(&mem, 0x16), Err("target_view"), "not made");
        mem.set_u32(descriptor + 0x88, 0x1000).unwrap();
        let header = descriptor + render_bridge::native_targets::TARGET_SURFACE;
        mem.set_u32(header + 0x0c, (0x12 << 8) | (2 << 4)).unwrap();
        mem.set_u32(header + 0x10, 639 | (479 << 12) | (39 << 24)).unwrap();
        assert_eq!(frame_target(&mem, 0x16), Ok(TargetRef::Game(0x16)));
        assert_eq!(frame_target(&mem, 0x100), Err("target_view"), "past the table");
    }
}
