//! The native fog's host (docs/graphics/native-water-fog.md, "Stage 4b"):
//! over the native world's loaded BSP (`native_world_glue`, which loads it
//! at the first layer 3 of a map), it compiles each material's layer-16
//! passes at the load and pushes one stage per view at the layer-16 call
//! (`render_bridge::native_fog`).
//!
//! - **At load** ([`load`], on the world's worker): each material's
//!   layer-16 passes (`native_render::fog_gpu::layer_passes_selected`,
//!   the planar gate's implementation, [`LOADED_IMPLEMENTATION`])
//!   compiled with their own programs ([`FogProgram::compile`]), all or
//!   nothing, their stage images decoded and their fog words kept; then
//!   the parts that can draw. One line says how many and why the rest do
//!   not.
//! - **Per view** ([`Host::ask`], at the layer-16 call): each material's
//!   own fog words resolve the fog. There is no device to hold the
//!   `D3DRS_FOG*` words a material leaves unset, so such a material
//!   panics by name.
//! - **The scene** (docs/graphics/framescene.md, slice 5): the resolved
//!   words and the parts the skip surrenders go into the view's
//!   `frame_scene::FogState` (`render_bridge::scene_step::late_layer`);
//!   the skip masks are written ([`commit_late`]) and the stage draws the
//!   scene's copy.
//! - **The stage** (`FogViewPass`): `GpuWorld::fog_draws` over the loaded
//!   depth, with the frame's view.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use engine::memory::GuestMemory;
use render_bridge::late_sorted::{Run, RunAt};
use render_bridge::native_fog::Host;
use render_bridge::native_layers::Left;
use map::{Map, TagId};
use native_render::fog_gpu::{self, FogProgram, FogWords, FOG_WORD_SLOTS};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::program::{self, ViewInputs};
use native_render::world::{LightmapKey, Owner, World};
use frame_scene::{LateMaterial, LatePart};

use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, State, current_ready};

/// The implementation of its layer-16 passes the load compiles: the one
/// layer 16's build binds for a record with the planar gate open
/// (`native_render::fog::layer_implementation`: 2). A pass with a single
/// implementation draws it for every item; an item whose record binds
/// another stays on the D3D path ([`FogView::draws`]).
pub const LOADED_IMPLEMENTATION: u8 = 2;

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
    /// Each drawn material's shader (tag index) that has a pass with more
    /// than one implementation: the implementation compiled for it. A
    /// material whose passes all have one is not here and draws for any
    /// item's implementation.
    pub chosen: HashMap<u16, u8>,
    /// Each drawn material's shader (tag index) with the implementation
    /// each of its layer-16 passes binds ([`LOADED_IMPLEMENTATION`] where
    /// a pass has more than one): what skipping its guest draw is checked
    /// against when a kept entry follows ([`FogView::skip_holds`]).
    pub implementations: HashMap<u16, Vec<tags::material::ResolvedImplementation>>,
}

impl FogView {
    /// Whether skipping the guest's draw of an item of `shader` leaves the
    /// device as drawing it would, over the device's render states
    /// (`read`, by guest state index): every pass's implementation holds
    /// (`layer_pass::skip_holds`). The sorted walk checks it, where a
    /// kept entry's draw follows the skipped ones.
    pub fn skip_holds(&self, shader: u16, read: &dyn Fn(u32) -> Option<u32>) -> bool {
        self.implementations
            .get(&shader)
            .is_some_and(|imps| imps.iter().all(|imp| native_render::layer_pass::skip_holds(imp, read)))
    }

    /// Whether the pass draws `part` of `owner` with `shader` (the item's
    /// `+8` low word) for an item whose record binds `implementation`
    /// (`FogItem::implementation`): the skip's acceptance.
    pub fn draws(&self, owner: Owner, part: u8, shader: u16, implementation: Option<u8>) -> bool {
        self.part_set.get(&(owner, part)) == Some(&shader)
            && self.chosen.get(&shader).is_none_or(|&loaded| implementation == Some(loaded))
    }
}

/// Compiles every material's layer-16 passes over the world (all of its
/// materials, not only the native world's: a material that stays on the
/// D3D path for layer 3 may still fog natively) and picks the parts that
/// can draw. Called from the world's worker, before its draws are kept.
pub fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> FogView {
    let mut programs: HashMap<u16, Vec<FogProgram>> = HashMap::new();
    let mut words: HashMap<u16, FogWords> = HashMap::new();
    let mut chosen: HashMap<u16, u8> = HashMap::new();
    let mut implementations = HashMap::new();
    let mut parts: Vec<((Owner, u8), (u32, u32, Option<LightmapKey>, u16))> = Vec::new();
    let materials: Vec<u16> = world.draws.iter().map(|d| d.material).collect::<BTreeSet<_>>().into_iter().collect();
    if !materials.is_empty() {
        match tags::material::ShaderLibrary::read(map) {
            Ok(mut lib) => {
                let mut refused: HashMap<String, usize> = HashMap::new();
                let mut rigid_refused: HashMap<String, usize> = HashMap::new();
                // Every material resolved first: whether any pass of the
                // layer sets a cull other than the layer's start decides
                // whether a pass that leaves it unset can be drawn
                // (`fog_gpu::layer_sets_other_cull`).
                let resolved: Vec<(u16, TagId, Result<tags::material::Material, String>)> = materials
                    .iter()
                    .filter_map(|&m| {
                        let &shader = shaders.get(usize::from(m))?;
                        Some((m, shader, lib.material(shader, None).map_err(|e| format!("material: {e}"))))
                    })
                    .collect();
                let rasterizer = map
                    .tags()
                    .iter()
                    .find(|tag| tag.group.0 == *b"matg")
                    .and_then(|tag| tag.location.as_ref())
                    .and_then(|loc| {
                        crate::native_lights_glue::matg_rasterizer_bitmap(
                            lib.metadata(),
                            loc.address,
                            crate::native_lights_glue::RASTERIZER_DATUM_14,
                        )
                    });
                let selection = fog_gpu::Selection {
                    rasterizer,
                    implementation: Some(usize::from(LOADED_IMPLEMENTATION)),
                    layer_cull: !resolved
                        .iter()
                        .filter_map(|(_, _, mat)| mat.as_ref().ok())
                        .any(|mat| fog_gpu::layer_sets_other_cull(mat, native_render::fog::FOG_LAYER)),
                };
                for (m, shader, mat) in resolved {
                    if mat.as_ref().is_ok_and(|mat| fog_gpu::layer_has_choice(mat, native_render::fog::FOG_LAYER)) {
                        chosen.insert(shader.0 as u16, LOADED_IMPLEMENTATION);
                    }
                    if let Some(imps) = mat.as_ref().ok().and_then(|mat| {
                        fog_gpu::layer_implementations(mat, native_render::fog::FOG_LAYER, Some(usize::from(LOADED_IMPLEMENTATION)))
                    }) {
                        implementations.insert(shader.0 as u16, imps);
                    }
                    let compiled = mat
                        .and_then(|mat| {
                            fog_gpu::layer_passes_selected(&mat, native_render::fog::FOG_LAYER, selection).map_err(|r| r.describe())
                        })
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
    FogView { programs, words, parts, part_set, chosen, implementations }
}

/// Builds the fog resources on the BSP loader's device.
pub fn upload_fog(
    fog: &FogView,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &World,
    stage_image: &mut dyn FnMut(Stage) -> Option<Image>,
    lightmap: &mut dyn FnMut(LightmapKey) -> Option<Image>,
) -> fog_gpu::FogSet {
    let set = fog_gpu::FogSet::new(device, queue, world, fog.parts.clone(), fog.programs.clone(), stage_image, lightmap);
    if hooks::native_verbose() {
        eprintln!("native-fog: {} materials' layer-16 passes uploaded, {} parts", set.len(), fog.parts.len());
    }
    set
}

/// The fog words the draws inherit at the layer-16 call, by
/// [`FOG_WORD_SLOTS`]: the guest's render-state shadow
/// (`native_targets_glue::render_state`). A material's own words win
/// over them ([`resolve_materials`]).
pub fn device_fog_words(mem: &dyn GuestMemory) -> [Option<u32>; 7] {
    fog_words_with(&|slot| crate::native_targets_glue::render_state(mem, slot))
}

/// [`device_fog_words`] from `states`, by guest state slot (a restore
/// list's: `native_render::sorted_entries::after_restore`).
pub fn fog_words_with(states: &dyn Fn(u32) -> Option<u32>) -> [Option<u32>; 7] {
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    FOG_WORD_SLOTS.map(|slot| table.canonical_to_guest(slot).and_then(states))
}

/// The fog unit and colour of resolved words (`gfx::guest_program::d3d_fog`).
pub fn fog_of(words: &[u32; 7]) -> (shaders::psh::fog::Fog, [f32; 4]) {
    (
        gfx::guest_program::d3d_fog(words[0] != 0, words[1], words[2], words[3], words[4], words[5] != 0).fog,
        shaders::psh::argb(words[6]),
    )
}

struct Glue {
    state: State,
}

/// A display-only part has no entry in the retail layer sort. The hook
/// supplies whole clusters only when there are no guest entries to order
/// against. Multiple candidates also need a sort before they can be drawn.
pub(super) fn display_late_part(
    mem: &dyn GuestMemory, clusters: &[u16], parts: &HashMap<(Owner, u8), u16>,
    layer: u8, eligible: impl Fn(u16, u16) -> bool,
) -> Option<(Owner, u8)> {
    let mut selected = None;
    for (&(owner, part), &loaded) in parts {
        let Owner::Cluster(cluster) = owner else { continue };
        let Ok(id) = u16::try_from(cluster) else { continue };
        if clusters.binary_search(&id).is_err() || !eligible(id, loaded) { continue; }
        if render_bridge::native_world::display_late_shader(mem, id, part, layer) != Some(loaded) {
            continue;
        }
        if selected.is_some() { return None; }
        selected = Some((owner, part));
    }
    selected
}

impl Host for Glue {
    fn ask(&self, mem: &mut dyn GuestMemory, display_clusters: &[u16], left: &mut Left) -> Option<native_render::skip::Cleared> {
        self.ask_at(mem, display_clusters, left, None, &mut Run::Refused)
    }

    fn ask_run(&self, mem: &mut dyn GuestMemory, at: &RunAt<'_>, left: &mut Left) -> Run {
        let mut run = Run::Refused;
        self.ask_at(mem, &[], left, Some(at), &mut run);
        run
    }
}

impl Glue {
    /// The ask before the layer's build (`at` `None`: the walk clears
    /// bits, `clear_fog`), or for one run of the sorted entries
    /// (`native_render::fog::sorted_run`, `render_bridge::late_sorted`),
    /// whose outcome goes to `out`.
    fn ask_at(
        &self, mem: &mut dyn GuestMemory, display_clusters: &[u16], left: &mut Left,
        at: Option<&RunAt<'_>>, out: &mut Run,
    ) -> Option<native_render::skip::Cleared> {
        use timeline::native::{Metric, Stage};
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(ready) = current_ready(&state, mem) else { return kept(left, "unloaded") };
            Arc::clone(ready)
        };
        let Some((fog, _)) = ready.fog.get() else { return kept(left, "unloaded") };
        let device_words = device_fog_words(mem);
        let Some(materials) = resolve_materials(&fog.words, &device_words) else {
            unimplemented!("fog: a planar fog word a material leaves to the render-state shadow does not read");
        };
        let read = ReadMemory(mem);
        let mut samples = fog_gpu::LayerSamples::new();
        let inputs = fog_gpu::PlanarInputs::read(&read);
        for (&material, passes) in &fog.programs {
            if passes.iter().any(|p| p.planar.is_some()) {
                let Some(inputs) = &inputs else {
                    return refuse_view(left, FOG_LATE_LAYER, "planar_inputs_view");
                };
                samples.insert(material, passes.iter().map(|p| {
                    p.planar.as_ref().map(|b| b.sample(&p.program.pixel, inputs))
                }).collect());
            }
        }
        let samples = Arc::new(samples);
        let pass = |selected, fogs| FogViewPass { ready: Arc::clone(&ready), selected, samples: Arc::clone(&samples), fogs };
        // A cluster only the display's wider view sees has no guest entry:
        // its part is a stage of its own.
        if !display_clusters.is_empty() {
            let Some(planar) = native_render::fog::planar_gate(&ReadMemory(mem)) else { return kept(left, "gate") };
            let selected = display_late_part(mem, display_clusters, &fog.part_set, FOG_LATE_LAYER, |id, shader| {
                let Some(record) = render_bridge::native_scene_lists::display_cluster_fog_flags(mem, id) else { return false };
                native_render::fog::layer_implementation(record, planar)
                    .is_some_and(|implementation| fog.chosen.get(&shader).is_none_or(|&loaded| loaded == implementation))
            });
            if let Some(selected) = selected {
                crate::native_targets_glue::push(&*mem, frame_types::Slot::AfterLights, pass(vec![selected], late_fogs(&materials)));
            }
        }
        let read = ReadMemory(mem);
        let Some(mut preview) = native_render::skip::MaskPreview::new(&read) else { return kept(left, "list") };
        let layer = native_render::fog::FOG_LAYER as u32;
        let (selected, cleared, run) = match at {
            None => {
                let Some((selected, cleared, refused)) = native_render::fog::clear_fog(&mut preview, layer, &|item| {
                    let accepted = fog.draws(item.owner, item.part, item.shader, item.implementation);
                    if !accepted {
                        Stage::Fog.count(Metric::Examined, 1);
                        Stage::Fog.count(Metric::RefusedOwnership, 1);
                    }
                    accepted
                }) else { return kept(left, "list") };
                count_late_refusals(layer, refused, left);
                (selected, cleared, None)
            }
            // A kept entry's draw follows the run, so skipping the run's
            // draws must leave the state as drawing them would: the state
            // the next implementation switch's restore list leaves.
            Some(at) => {
                let Some(restore) = native_render::sorted_entries::cached_restore(&read) else { return kept(left, "cached_state") };
                let state = native_render::sorted_entries::after_restore(&restore, &|_| None);
                let Some(run) = native_render::fog::sorted_run(&read, layer, &at.sorted, at.from, &|item| {
                    if !fog.draws(item.owner, item.part, item.shader, item.implementation) {
                        Err("material")
                    } else if !fog.skip_holds(item.shader, &state) {
                        Err("state")
                    } else {
                        Ok(())
                    }
                }) else { return kept(left, "list") };
                run_stop(layer, run.stop, left);
                if run.drawn.is_empty() {
                    *out = Run::Stopped;
                    return None;
                }
                let cleared = native_render::skip::Cleared { layer, items: Vec::new() };
                (run.drawn.clone(), cleared, Some((at, run)))
            }
        };
        let Some(masks) = preview.finish() else { return kept(left, "list") };
        if selected.is_empty() {
            return None;
        }
        // The view's fog into the frame step's scene, the layer's earlier
        // runs first; the stage draws the scene's copy of this run.
        let prior = run.as_ref().map_or(&[][..], |(at, _)| at.prior);
        let mut parts: Vec<LatePart> = prior.to_vec();
        parts.extend(selected.iter().map(|item| LatePart { owner: item.owner, part: item.part }));
        let Some(late) = render_bridge::scene_step::late_layer(&*mem, FOG_LATE_LAYER, true, device_words, &materials, &parts) else {
            return kept(left, "scene");
        };
        let fogs = late_fogs(&late.materials);
        let Some(this_run) = late.parts.get(prior.len()..) else { return kept(left, "scene") };
        let selected = late_selected(this_run);
        if !commit_late(mem, &self.state, &ready, pass(selected, fogs), &masks) {
            return kept(left, "reservation");
        }
        render_bridge::scene_step::late_drawn(late.key, FOG_LATE_LAYER);
        if let Some((_, run)) = run {
            *out = Run::Drawn { slots: run.slots, entries: run.entries, parts: this_run.to_vec() };
        }
        Some(cleared)
    }
}

/// The entry that ended a sorted run keeps its guest draw: its cause named
/// for its slot, and counted `layer_plan_<layer>_refused_<cause>` when the
/// run decided it (`state`, `entry`); the walk before the build counted
/// the rest.
pub(super) fn run_stop(layer: u32, stop: Option<(u32, &'static str)>, left: &mut Left) {
    let Some((slot, why)) = stop else { return };
    left.item(slot, why);
    if matches!(why, "state" | "entry") {
        render_bridge::native_layers::count(layer, &format!("refused_{why}"), 1);
    }
}

/// The fog's layer, as the scene names it.
const FOG_LATE_LAYER: u8 = native_render::fog::FOG_LAYER as u8;

/// Each material's own fog words resolved over the device's, ordered by
/// material (`frame_scene::LateMaterial`); `None` when a slot nothing sets
/// does not read.
pub fn resolve_materials(words: &HashMap<u16, FogWords>, device: &[Option<u32>; 7]) -> Option<Vec<LateMaterial>> {
    let mut out = words
        .iter()
        .map(|(&material, w)| Some(LateMaterial { material, words: w.resolve(device)? }))
        .collect::<Option<Vec<_>>>()?;
    out.sort_by_key(|m| m.material);
    Some(out)
}

/// The scene's resolved materials as the fog unit and colour the passes
/// draw with.
pub fn late_fogs(materials: &[LateMaterial]) -> HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])> {
    materials.iter().map(|m| (m.material, fog_of(&m.words))).collect()
}

/// The scene's parts as the GPU world names them.
pub fn late_selected(parts: &[LatePart]) -> Vec<(Owner, u8)> {
    parts.iter().map(|p| (p.owner, p.part)).collect()
}

/// The view's whole layer keeps its guest draw under `cause`
/// (`scene_kept_<layer>_<kind>_<cause>`, `render_bridge::native_fog::count_late_kept`).
pub(super) fn kept(left: &mut Left, cause: &'static str) -> Option<native_render::skip::Cleared> {
    left.all(cause);
    None
}

/// A view refusal: counted `layer_plan_<layer>_refused_<cause>`, and the
/// view's items kept under `cause`.
pub(super) fn refuse_view(left: &mut Left, layer: u8, cause: &'static str) -> Option<native_render::skip::Cleared> {
    render_bridge::native_layers::count(layer.into(), &format!("refused_{cause}"), 1);
    kept(left, cause)
}

/// The walk's refusals: counted `layer_plan_<layer>_refused_<cause>`, and
/// each item's cause named for its slot in `left`.
///
/// An `order` item is not counted: the bridge draws it per item over the
/// sorted entries (`render_bridge::late_sorted`), and what it keeps is
/// counted there by its own cause.
pub(super) fn count_late_refusals(layer: u32, refused: native_render::fog::LateRefusals, left: &mut Left) {
    for &(slot, why) in &refused.kept {
        left.item(slot, why);
    }
    for (cause, n) in [
        ("refused_build", refused.build),
        ("refused_handler", refused.handler),
        ("refused_record", refused.record),
        ("refused_object", refused.object),
        ("refused_subparts", refused.subparts),
        ("refused_material", refused.material),
    ] {
        render_bridge::native_layers::count(layer, cause, n);
    }
}

/// A late layer's stage committed (docs/graphics/framescene.md, slice 5):
/// while `ready` is still the current load, the skip masks are written
/// and `stage` is pushed. `false`: nothing was committed, and the guest's
/// layer is left as it was.
pub fn commit_late(
    mem: &mut dyn GuestMemory,
    state: &State,
    ready: &Arc<Ready>,
    stage: impl native_render::Stage + 'static,
    masks: &native_render::skip::MaskPlan,
) -> bool {
    {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        if !current_ready(&state, mem).is_some_and(|current| Arc::ptr_eq(current, ready)) {
            return false;
        }
    }
    if !masks.apply(&mut render_bridge::native_fog::Mem(mem)) {
        return false;
    }
    crate::native_targets_glue::push(&*mem, frame_types::Slot::AfterLights, stage);
    true
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub fn install(state: State) {
    render_bridge::native_fog::set_host(Arc::new(Glue { state }));
}

/// One view's fog.
struct FogViewPass {
    ready: Arc<Ready>,
    /// Parts in layer order.
    selected: Vec<(Owner, u8)>,
    samples: Arc<fog_gpu::LayerSamples>,
    /// Each material's fog unit and colour, resolved at the ask.
    fogs: HashMap<u16, (shaders::psh::fog::Fog, [f32; 4])>,
}

impl native_render::Stage for FogViewPass {
    fn name(&self) -> &'static str {
        "fog"
    }

    /// The fog's draws join the render pass open on the attachments,
    /// their uniforms written ahead of them (`ExternalDraws::writes`).
    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &ViewInputs) -> native_render::StageWork {
        if view.w_clip.is_none() {
            unimplemented!("fog: the view's W-buffer clip");
        }
        let mut guard = self.ready.gpu();
        let Some(gpu) = guard.as_mut() else { unimplemented!("fog: world geometry unavailable") };
        let viewport = [0.0, 0.0, t.size.0 as f32, t.size.1 as f32];
        match gpu.fog_draws(&t.device, t.color_format, t.depth_format, t.size, viewport, &self.selected, view, &self.fogs, &self.samples) {
            Ok(draws) => native_render::StageWork::Draws(draws),
            Err(why) => unimplemented!("fog: {why}"),
        }
    }
}

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
        // `gfx::guest_program::d3d_fog` is the authority; a smoke check that the words reach
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
