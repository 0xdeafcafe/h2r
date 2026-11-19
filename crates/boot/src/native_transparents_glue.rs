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
//!   object's when its loaded model, record section and part match and
//!   its record's node rows read (`native_objects_glue::Models`).
//! - **Per run** ([`Host::open`]): one external pass. The device issues it
//!   at its next entry; it takes the run's entries then
//!   (`ExternalPass::view_constants`, which seals the run) and draws them
//!   in order, consecutive world entries with `GpuWorld::encode_transparents`
//!   and consecutive objects with `LitObjects::encode_transparents`.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use engine::memory::GuestMemory;
use feature::native_transparents::Host;
use gfx::state::{DepthBufferMode, RenderState, RenderStateType as R};
use map::{Map, TagId};
use native_render::camera::{GuestRead, RENDER_CAMERA, RenderCamera, transform};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::objects::ObjectFrame;
use native_render::pass_state::{PassState, TRANSPARENT_INHERITED};
use native_render::program::{RigidUniforms, ViewInputs};
use native_render::transparent::{Entry, Run, RunEntry, Source};
use native_render::transparent_gpu::{Programs, SectionKind, TransparentProgram};
use native_render::world::{LightmapKey, Owner, World};

use crate::native_objects_glue::{Models, current_models, upload_pending, want_transparent};
use crate::native_world_glue::{
    Load, ReadMemory, Ready, StageDecoder, StageImages, State, active_bsp_index, covers, palettes_hold, parse_probe, upload,
};

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
    fn ready(&self, mem: &dyn GuestMemory) -> Option<Arc<Ready>> {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (name, bsp, index) = st.0.as_ref()?;
        if crate::health::loaded_map_name(mem).ok().as_ref() != Some(name)
            || Some(*index) != active_bsp_index(mem)
            || Some(*bsp) != mem.u32(feature::native_world::ACTIVE_BSP).ok()
        {
            return None;
        }
        match &st.1 {
            Load::Ready(r) => Some(Arc::clone(r)),
            Load::Idle | Load::Loading | Load::Failed => None,
        }
    }

    fn queue(
        &self,
        mem: &dyn GuestMemory,
        camera: &RenderCamera,
        run: Arc<Run>,
        compare: bool,
    ) -> bool {
        let (ready, models) = (self.ready(mem), current_models());
        if ready.is_none() && models.is_none() {
            return false;
        }
        let Ok(mut d) = self.device.lock() else { return false };
        if !d.can_external_pass() {
            return false;
        }
        let Some(incoming) = incoming_state(&|slot| d.render_state(&ReadMemory(mem), slot)) else {
            if !UNREAD_STATE.swap(true, Ordering::Relaxed) {
                eprintln!("native-transparents: incoming render states could not be read (logged once)");
            }
            return false;
        };
        let w = d.depth_clip(&ReadMemory(mem), 1.0).w_buffered;
        if incoming.depth_mode != DepthBufferMode::W || w == Some(false) {
            if !NON_W_BUFFERED.swap(true, Ordering::Relaxed) {
                eprintln!("native-transparents: leaving draws to D3D in a non-W view (logged once)");
            }
            return false;
        }
        d.request_external_pass(Box::new(RunPass {
            ready,
            models,
            camera: *camera,
            run,
            view: None,
            clip: None,
            segments: Vec::new(),
            incoming,
            compare,
            traced: false,
        }));
        true
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn prepare_object(&self, mem: &dyn GuestMemory, entry: &Entry, frame: &mut ObjectFrame) -> bool {
        let Some(models) = want_transparent(mem, frame.definition) else { return false };
        let record = native_render::skip::RENDER_RECORDS + native_render::skip::RENDER_RECORD_SIZE * u32::from(entry.record);
        let Some(key) = mem.u32(record + native_render::object_nodes::RECORD_KEY).ok() else { return false };
        let Some(model) = mem.u32(record + native_render::object_nodes::RECORD_MODEL).ok() else { return false };
        let section = ((key >> 9) & 0x1ff) as u16;
        let Some(implementation) = models.transparent_implementation(frame, model, section, entry) else { return false };
        let Ok(rows) = native_render::object_nodes::node_rows(&ReadGuest(mem), record) else { return false };
        if rows.iter().flatten().any(|v| !v.is_finite()) {
            return false;
        }
        frame.node_rows.push((section, rows));
        frame.implementations.push((section, implementation));
        true
    }

    fn accepts(&self, mem: &dyn GuestMemory, entry: &Entry, object: Option<&ObjectFrame>) -> bool {
        let ready = if object.is_some() { None } else { self.ready(mem) };
        let models = if object.is_some() { current_models() } else { None };
        {
            let Ok(d) = self.device.lock() else { return false };
            let bound = |s| d.stage_palette(&ReadMemory(mem), s);
            let accepted = match object {
                Some(_) => models.is_some_and(|m| m.transparent_palettes_hold(&bound)),
                None => ready.is_some_and(|r| accepts_entry(&r.transparent_parts, entry) && palettes_hold(&r.palettes, &bound)),
            };
            if !accepted {
                return false;
            }
        }
        if let Some(dump) = diagnostics().compare.filter(|d| d.matches(entry)) {
            let seen = MATCHES.fetch_add(1, Ordering::Relaxed);
            if seen == dump.after && let Some(camera) = RenderCamera::read(&ReadGuest(mem), RENDER_CAMERA) {
                let run = Arc::new(Run::new());
                run.push(*entry, object.cloned());
                if !self.queue(mem, &camera, run, true) {
                    eprintln!("native-transparents compare {:?} part {}: device state unavailable", entry.source, entry.part);
                }
            }
            // Leave this entry to D3D so the queued comparison sees its draw.
            return false;
        }
        true
    }

    fn open(&self, mem: &dyn GuestMemory, camera: &RenderCamera, run: Arc<Run>) -> bool {
        self.queue(mem, camera, run, false)
    }
}

struct ReadGuest<'a>(&'a dyn GuestMemory);

impl GuestRead for ReadGuest<'_> {
    fn read_u32(&self, address: u32) -> Option<u32> {
        self.0.u32(address).ok()
    }
}

fn incoming_state(read: &dyn Fn(u32) -> Option<u32>) -> Option<RenderState> {
    static TABLE: OnceLock<gfx::xdk::RenderStateTable> = OnceLock::new();
    let table = TABLE.get_or_init(gfx::xdk::RenderStateTable::default);
    let mut state = RenderState::default();
    for t in TRANSPARENT_INHERITED.into_iter().chain([R::ZEnable]) {
        let slot = table.canonical_to_guest(t.index())?;
        state.set_raw(t, read(slot)?).ok()?;
    }
    Some(state)
}

/// Installs the host over the world's state (`native_world_glue::install`).
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: State) {
    feature::native_transparents::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        state,
    }));
}

enum Segment {
    World(Vec<Entry>),
    Objects(Vec<(Entry, ObjectFrame)>),
}

fn segments(entries: Vec<RunEntry>) -> Vec<Segment> {
    let mut out = Vec::new();
    for RunEntry { entry, object } in entries {
        match object {
            None => match out.last_mut() {
                Some(Segment::World(v)) => v.push(entry),
                _ => out.push(Segment::World(vec![entry])),
            },
            Some(frame) => match out.last_mut() {
                Some(Segment::Objects(v)) => v.push((entry, frame)),
                _ => out.push(Segment::Objects(vec![(entry, frame)])),
            },
        }
    }
    out
}

struct RunPass {
    ready: Option<Arc<Ready>>,
    models: Option<Arc<Models>>,
    camera: RenderCamera,
    run: Arc<Run>,
    view: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    segments: Vec<Segment>,
    incoming: RenderState,
    compare: bool,
    traced: bool,
}

impl d3d::ExternalPass for RunPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.view = Some(*constants);
        self.segments = segments(self.run.seal());
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let Some(constants) = self.view else { return };
        let view = ViewInputs {
            constants,
            guest_size: t.guest_size,
            size: t.size,
            z_scale: t.z_scale,
            w_clip: self.clip.map(|c| (c.range, c.clamp)),
        };
        let trace = !std::mem::replace(&mut self.traced, true);
        let probe = diagnostics().probe.filter(|_| trace);
        let compare = self.compare && trace;
        for segment in &self.segments {
            match segment {
                Segment::World(entries) => {
                    let Some(ready) = &self.ready else { continue };
                    let mut gpu = upload(ready, &t);
                    let Some(gpu) = gpu.as_mut() else { continue };
                    let world_entries: Vec<_> = entries.iter().filter_map(|e| {
                        owner(e.source).map(|o| (o, e.part))
                    }).collect();
                    if compare || probe.is_some() {
                        for entry in entries {
                            let Some(draw) = owner(entry.source).and_then(|o| gpu.world().transparent(o, entry.part)) else { continue };
                            if let Some(at) = probe {
                                probe_world(gpu.world(), draw, &self.camera, at, entry);
                            }
                            if compare && let Some(set) = gpu.transparents() {
                                arm(entry, set.uniforms(gpu.world(), draw, &view, &self.incoming));
                            }
                        }
                    }
                    if !self.compare {
                        gpu.encode_transparents(
                            t.device, t.encoder, t.color, t.color_format, t.depth, t.depth_format,
                            t.size, &view, &world_entries, &self.incoming,
                        );
                    }
                }
                Segment::Objects(entries) => {
                    let Some(models) = &self.models else { continue };
                    drop(upload_pending(models, &t));
                    let index = models.index.lock().unwrap_or_else(|e| e.into_inner());
                    let mut gpu = models.transparent_gpu.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(gpu) = gpu.as_mut() else { continue };
                    let items: Vec<_> = entries.iter()
                        .filter_map(|(entry, o)| Some((*index.get(&(o.definition as u16))?, o, entry)))
                        .collect();
                    drop(index);
                    if compare || probe.is_some() {
                        for &(model, frame, entry) in &items {
                            let Some((uniforms, state)) = gpu.transparent_uniforms(model, frame, entry.part, &view, &self.incoming) else { continue };
                            if let Some(at) = probe
                                && let Some(triangles) = gpu.transparent_triangles(model, frame, entry.part, &uniforms)
                            {
                                probe_hits(at, entry, triangles);
                            }
                            if compare {
                                arm(entry, vec![(uniforms, state)]);
                            }
                        }
                    }
                    if !self.compare {
                        let items: Vec<_> = items.into_iter().map(|(model, frame, entry)| (model, frame, entry.part)).collect();
                        gpu.encode_transparents(
                            t.device, t.queue, t.encoder, t.color, t.color_format, t.depth, t.depth_format,
                            t.size, &view, &items, &self.incoming,
                        );
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Compare {
    source: Source,
    part: u8,
    record: Option<u16>,
    after: u64,
}

impl Compare {
    fn matches(&self, entry: &Entry) -> bool {
        self.source == entry.source
            && self.part == entry.part
            && self.record.is_none_or(|r| entry.record == r)
    }
}

fn parse_compare(v: &str) -> Option<Compare> {
    let (v, after) = match v.split_once('@') {
        Some((v, after)) => (v, after.trim().parse().ok()?),
        None => (v, 0),
    };
    let mut fields = v.split(':').map(str::trim);
    let kind = fields.next()?;
    let id = fields.next()?;
    let part = fields.next()?.parse().ok()?;
    let record = fields.next().map(str::parse).transpose().ok()?;
    if fields.next().is_some() {
        return None;
    }
    let source = match kind {
        "cluster" => Source::Cluster(id.parse().ok()?),
        "placement" => Source::Placement(id.parse().ok()?),
        "object" => Source::Object(match id.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => id.parse().ok()?,
        }),
        _ => return None,
    };
    Some(Compare { source, part, record, after })
}

struct Diagnostics {
    compare: Option<Compare>,
    probe: Option<(f32, f32)>,
}

static MATCHES: AtomicU64 = AtomicU64::new(0);
static UNREAD_STATE: AtomicBool = AtomicBool::new(false);
static NON_W_BUFFERED: AtomicBool = AtomicBool::new(false);

fn diagnostics() -> &'static Diagnostics {
    static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();
    DIAGNOSTICS.get_or_init(|| Diagnostics {
        compare: std::env::var("H2_NATIVE_TRANSPARENTS_COMPARE").ok().as_deref().and_then(parse_compare),
        probe: std::env::var("H2_NATIVE_TRANSPARENTS_PROBE").ok().as_deref().and_then(parse_probe),
    })
}

fn arm(entry: &Entry, passes: Vec<(RigidUniforms, PassState)>) {
    if passes.is_empty() {
        eprintln!("native-transparents compare {:?} part {}: native candidate unavailable", entry.source, entry.part);
        return;
    }
    let candidates: Vec<_> = passes.into_iter().enumerate().map(|(n, (u, state))| {
        let label = format!(
            "transparent {:?} part {} record {} shader {:#010x} pass {n}",
            entry.source, entry.part, entry.record, entry.shader,
        );
        eprintln!(
            "native-transparents compare {label}: {} program words, depth clip {:?} clamp {}, state {state:?}",
            u.words.len(), u.vertex.w_clip, u.vertex.w_clamp
        );
        d3d::uniform_compare::Expected {
            label,
            words: u.words,
            vertex: u.vertex,
            pixel: u.pixel,
            samplers: u.samplers,
            options: u.options,
            def: u.def,
            pixel_options: u.pixel_options,
            textures: u.textures,
            primitive: u.pipeline.0,
            depth: (u.pipeline.1.0, u.pipeline.1.1, Default::default()),
            color: Some(d3d::uniform_compare::ColorState {
                blend: state.blend,
                write_mask: state.color_writes,
                blend_constant: state.blend_constant,
            }),
            draws: 1,
        }
    }).collect();
    d3d::uniform_compare::arm_entry(candidates, !matches!(entry.source, Source::Cluster(_)));
}

fn probe_world(
    world: &World,
    draw: &native_render::world::TransparentDraw,
    camera: &RenderCamera,
    at: (f32, f32),
    entry: &Entry,
) {
    let vp = camera.view_projection();
    let [top, left, bottom, right] = camera.viewport.map(f32::from);
    let project = |i: u32| {
        let [x, y, z] = world.vertices[i as usize].position;
        let [x, y, _, w] = transform(&vp, [x, y, z, 1.0]);
        (w > 0.0).then(|| (
            left + (x / w + 1.0) * 0.5 * (right - left),
            top + (1.0 - y / w) * 0.5 * (bottom - top),
            w,
        ))
    };
    let indices = &world.indices[draw.first_index as usize..(draw.first_index + draw.index_count) as usize];
    probe_hits(at, entry, indices.chunks_exact(3).filter_map(|t| Some([project(t[0])?, project(t[1])?, project(t[2])?])));
}

fn probe_hits(
    at: (f32, f32),
    entry: &Entry,
    triangles: impl IntoIterator<Item = [(f32, f32, f32); 3]>,
) {
    let pixel = (at.0 + 0.5, at.1 + 0.5);
    if let Some((depth, edge)) = triangles.into_iter().filter_map(|tri| covers(&tri, pixel))
        .min_by(|a, b| a.0.total_cmp(&b.0))
    {
        eprintln!(
            "native-transparents probe {at:?}: {:?} part {} record {} shader {:#010x} sort depth {:.3}, triangle depth {depth:.3}, edge {edge:.2} px",
            entry.source, entry.part, entry.record, entry.shader, entry.depth
        );
    }
}

/// At load: the transparent materials of `world` (by BSP material index;
/// `shaders` the BSP's shader per material) whose layer-15 passes all
/// translate, their stages' images added to `images` through `decoder`
/// (a palettized bitmap expanded through the device palette at its stage,
/// as the world's are), and the parts the native pass can draw with their
/// shader's tag index. Says on stderr how many and why the rest stay on
/// the D3D path.
pub(crate) fn load<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shaders: &[TagId],
    world: &World,
    lightmaps: &HashMap<LightmapKey, Image>,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> (Programs, HashMap<(Owner, u8), u16>) {
    let mut programs = Programs::new();
    let mut parts = HashMap::new();
    let materials: BTreeSet<_> = world.transparents.iter()
        .map(|t| (t.material, SectionKind::from(t.owner))).collect();
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
    for &(m, kind) in &materials {
        let Some(&shader) = shaders.get(usize::from(m)) else { continue };
        let compiled = lib
            .material(shader, None)
            .map_err(|e| format!("material: {e}"))
            .and_then(|mat| look::transparent_passes(&mat).map_err(|r| format!("{r:?}")))
            .and_then(|passes| {
                passes
                    .iter()
                    .map(|p| {
                        let code = match kind {
                            SectionKind::Cluster => look::world_program(map, lib.metadata(), p.look.vertex_shader),
                            SectionKind::Placement => look::rigid_program(map, lib.metadata(), p.look.vertex_shader),
                        }.ok_or("no vertex program for this classification")?;
                        TransparentProgram::compile(p, &code)
                    })
                    .collect::<Result<Vec<_>, String>>()
            })
            .and_then(|passes| {
                // Every stage image, or the material stays on the D3D path.
                for p in &passes {
                    for (n, s) in p.program.stages.iter().enumerate() {
                        if let Stage::Bitmap { bitmap, image } = *s
                            && p.program.dims[n].is_some()
                        {
                            decoder.decode(map, files, images, bitmap, image, n, true)?;
                        }
                    }
                }
                Ok(passes)
            });
        match compiled {
            Ok(p) => {
                programs.insert((m, kind), p);
            }
            Err(why) => *refused.entry(why).or_default() += 1,
        }
    }
    let mut unlit = 0;
    for t in &world.transparents {
        let Some(passes) = programs.get(&(t.material, t.owner.into())) else { continue };
        if passes.iter().any(|p| p.program.inputs & !t.inputs != 0)
            || matches!(t.owner, Owner::Instance(i) if world.rows(i).is_none())
        {
            *refused.entry("a part lacks a program stream or placement rows".into()).or_default() += 1;
            continue;
        }
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
        "native-transparents: {} of {} transparent material/classification pairs translate, {} of {} parts native ({unlit} without their lightmap page); on the D3D path: {why:?}",
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
        Entry { shader, record: 7, part, source, depth: 0.0, position: [0.0; 3] }
    }

    #[test]
    fn entries_are_accepted_by_their_part_and_shader() {
        let parts = [((Owner::Cluster(37), 2), 0x42), ((Owner::Instance(140), 0), 0x43)].into_iter().collect();
        assert!(accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Cluster(37))));
        assert!(accepts_entry(&parts, &entry(0xe123_0043, 0, Source::Placement(140))));
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 1, Source::Cluster(37))));
        assert!(!accepts_entry(&parts, &entry(0xe123_0044, 2, Source::Cluster(37))));
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Placement(37))));
        assert!(!accepts_entry(&parts, &entry(0xe123_0042, 2, Source::Object(0xe1a3_0004))));
    }

    #[test]
    fn transparent_segments_keep_world_object_world_order() {
        let frame = ObjectFrame {
            handle: 12,
            definition: 3,
            nodes: Vec::new(),
            sphere: ([0.0; 3], 1.0),
            lighting: None,
            defaults: None,
            node_rows: Vec::new(),
            implementations: Vec::new(),
        };
        let first = entry(4, 0, Source::Cluster(2));
        let object = entry(5, 2, Source::Object(12));
        let another_part = entry(5, 1, Source::Object(12));
        let last = entry(6, 3, Source::Placement(3));
        let sequence = segments(vec![
            RunEntry { entry: first, object: None },
            RunEntry { entry: object, object: Some(frame.clone()) },
            RunEntry { entry: another_part, object: Some(frame) },
            RunEntry { entry: last, object: None },
        ]);
        let [Segment::World(a), Segment::Objects(b), Segment::World(c)] = sequence.as_slice() else {
            panic!("expected world, objects, world segments");
        };
        assert_eq!(a, &[first]);
        assert_eq!(b.iter().map(|(entry, _)| *entry).collect::<Vec<_>>(), [object, another_part]);
        assert_eq!(c, &[last]);
    }

    #[test]
    fn a_transparent_run_reads_the_game_numbered_incoming_states() {
        use gfx::state::{BlendFactor, CompareFunc, CullMode, FrontFace};
        let table = gfx::xdk::RenderStateTable::default();
        let values = [
            (R::CullMode, CullMode::Ccw.xbox_value()),
            (R::FrontFace, FrontFace::Cw.xbox_value()),
            (R::ZFunc, CompareFunc::Equal.xbox_value()),
            (R::ZWriteEnable, 0),
            (R::AlphaBlendEnable, 1),
            (R::SrcBlend, BlendFactor::SrcAlpha.xbox_value()),
            (R::DestBlend, BlendFactor::InvSrcAlpha.xbox_value()),
            (R::ZEnable, DepthBufferMode::W.xbox_value()),
        ];
        let mut slots: HashMap<_, _> = values.into_iter().map(|(t, v)| (table.canonical_to_guest(t.index()).unwrap(), v)).collect();
        let state = incoming_state(&|slot| slots.get(&slot).copied()).unwrap();
        assert_eq!(state.cull_mode, CullMode::Ccw);
        assert_eq!(state.depth_func, CompareFunc::Equal);
        assert_eq!(state.depth_mode, DepthBufferMode::W);
        assert!(state.blend_enable);
        assert!(!state.depth_write);
        slots.remove(&table.canonical_to_guest(R::FrontFace.index()).unwrap());
        assert!(incoming_state(&|slot| slots.get(&slot).copied()).is_none());
    }

    #[test]
    fn transparent_compare_selects_an_exact_sorted_entry() {
        let compare = parse_compare("object:0x80010002:3:7@42").unwrap();
        assert_eq!(compare.after, 42);
        assert!(compare.matches(&entry(17, 3, Source::Object(0x8001_0002))));
        assert!(!compare.matches(&entry(17, 4, Source::Object(0x8001_0002))));
        let other_record = Entry { record: 8, ..entry(17, 3, Source::Object(0x8001_0002)) };
        assert!(!compare.matches(&other_record));
        assert_eq!(parse_compare("cluster:2:1").unwrap().source, Source::Cluster(2));
        assert_eq!(parse_compare("placement:2:1").unwrap().source, Source::Placement(2));
        assert_eq!(parse_compare("cluster:65536:1"), None);
        assert_eq!(parse_compare("placement:1:256"), None);
        assert_eq!(parse_compare("object:1:1@"), None);
    }
}
