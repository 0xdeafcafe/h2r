//! The host of `render_bridge::water_end`'s ocean draw
//! (docs/graphics/retire-d3d-display.md, "The water end"): the BSP
//! ocean's layer-3 pass compiled from the map (`native_render::ocean_gpu`)
//! and drawn as a water scene stage on the bound target.
//!
//! | Step | Where |
//! |---|---|
//! | shader, material, `vrtx`, bitmaps | the world's worker, [`load`] |
//! | compile for the grid stream's format | the first draw, on the guest thread (the format table is guest memory) |
//! | grid vertices and quads | each draw, uploaded again for a new stream or quad list |
//! | targets `0x15`, `0x16` | the frame graph's surfaces for their roles (`TargetRef::Game`, [`sampled_targets`]), sampled in place ([`native_render::Stage::samples`]) |
//!
//! What it cannot make is a named cause (`water ocean: <cause>`), never
//! a guest draw.

use std::io::{Read, Seek};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use map::{Map, TagId};
use native_render::gpu::Image;
use native_render::look::{self, Stage};
use native_render::object_streams::StreamFormats;
use native_render::ocean_gpu::{self, OceanDraw, OceanGrid, OceanTarget, OceanProgram, OceanSet, OceanSource};
use render_bridge::water_end::{self, ocean};
use tags::material::{Material, TextureSource};
use tags::shader::AnimatedProperty;

use frame_types::{Slot, TargetRef};
use crate::native_world_glue::{ReadMemory, Ready, StageDecoder, StageImages, StagePalettes, State, current_ready, };

/// The ocean of one loaded BSP: what its pass compiles from.
pub struct OceanData {
    /// The water definition's shader.
    pub shader: TagId,
    pub material: Material,
    pub properties: Vec<AnimatedProperty>,
    pub rasterizer: Option<TagId>,
    /// The pass's `vrtx` rigid classification.
    pub microcode: Vec<u8>,
    pub semantics: Vec<u16>,
    /// The bitmap stages' images, decoded through the load's palettes.
    pub images: Vec<(Stage, Image)>,
    pub palettes: StagePalettes,
}

/// The ocean of the BSP `bsp`: `None` when it has no water definition,
/// else the data or why it cannot be drawn.
pub fn load<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    bsp: TagId,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> Option<Result<OceanData, String>> {
    let shader = match native_render::load::ocean_shader(map, bsp) {
        Ok(Some(shader)) => shader,
        Ok(None) => return None,
        Err(e) => return Some(Err(format!("water definitions: {e}"))),
    };
    let data = load_shader(map, files, shader, images, decoder);
    if let Err(why) = &data {
        eprintln!("native-ocean: the ocean shader does not load ({why}); its draw panics as `water ocean: program`");
    }
    Some(data)
}

fn load_shader<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    shader: TagId,
    images: &mut StageImages,
    decoder: &mut StageDecoder,
) -> Result<OceanData, String> {
    let mut lib = tags::material::ShaderLibrary::read(map).map_err(|e| format!("shader library: {e}"))?;
    let rasterizer = map
        .tags()
        .iter()
        .find(|tag| tag.group.0 == *b"matg")
        .and_then(|tag| tag.location)
        .and_then(|loc| {
            crate::native_lights_glue::matg_rasterizer_bitmap(lib.metadata(), loc.address, crate::native_lights_glue::RASTERIZER_DATUM_14)
        });
    let material = lib.material(shader, None).map_err(|e| format!("material: {e}"))?;
    let properties = native_render::layer_pass::instance_properties(&lib, shader);
    let (_, imp) = ocean_gpu::implementation(&material)?;
    let vrtx = imp.vertex_shader.ok_or("layer 3 pass 0 has no vertex shader")?;
    let address = map
        .tag(vrtx)
        .ok()
        .and_then(|t| t.location)
        .map(|l| l.address)
        .ok_or("the vertex shader tag has no address")?;
    let microcode = look::vertex_program(lib.metadata(), address, look::RIGID_CLASSIFICATION)
        .ok_or("the vertex shader has no rigid program")?;
    let semantics = look::vertex_semantics(lib.metadata(), address, look::RIGID_CLASSIFICATION)
        .ok_or("the rigid program's inputs do not read")?;
    let (supplied, _) = ocean_gpu::supplied(imp, rasterizer)?;
    let mut stages = Vec::new();
    for (n, t) in supplied.textures.iter().enumerate() {
        let Some(TextureSource::Bitmap { bitmap, image, .. }) = t.as_ref().map(|t| t.source) else { continue };
        decoder.decode(map, files, images, bitmap, image, n, false)?;
        let decoded = images.get(&(bitmap, image)).cloned().ok_or_else(|| format!("stage {n} bitmap did not decode"))?;
        stages.push((Stage::Bitmap { bitmap, image }, decoded));
    }
    Ok(OceanData {
        shader,
        material,
        properties,
        rasterizer,
        microcode,
        semantics,
        images: stages,
        palettes: decoder.used().clone(),
    })
}

/// The ocean set on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, OceanSet)>> = Mutex::new(None);

/// The compiled pass and the grid of the last draw, for the world they
/// were made over (its `Ready`'s address) and the stream format.
#[derive(Default)]
struct Cached {
    program: Option<(usize, u8, Arc<OceanProgram>)>,
    grid: Option<Arc<OceanGrid>>,
}

struct Glue {
    state: State,
    cached: Mutex<Cached>,
}

/// The guest's bytes at `[at, at + len)`.
fn guest_bytes(mem: &dyn GuestMemory, at: u32, len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(len + 3);
    let mut a = at;
    while out.len() < len {
        out.extend_from_slice(&mem.u32(a).ok()?.to_le_bytes());
        a = a.checked_add(4)?;
    }
    out.truncate(len);
    Some(out)
}

/// What a grid's bytes are read from: the stream's vertex address and
/// the quads.
fn grid_key(vertices: u32, quads: &[[u16; 4]]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vertices.hash(&mut h);
    quads.hash(&mut h);
    h.finish()
}

impl Glue {
    fn program(&self, ready: &Arc<Ready>, data: &OceanData, mem: &dyn GuestMemory, kind: u8) -> Result<Arc<OceanProgram>, ocean::Cause> {
        let world = Arc::as_ptr(ready) as usize;
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((w, k, p)) = &cached.program
            && (*w, *k) == (world, kind)
        {
            return Ok(Arc::clone(p));
        }
        let formats = StreamFormats::read(&ReadMemory(mem)).ok_or("stream")?;
        let stream = formats.elements(kind).map_err(|_| "stream")?;
        let src = OceanSource {
            material: &data.material,
            properties: &data.properties,
            rasterizer: data.rasterizer,
            microcode: &data.microcode,
            semantics: &data.semantics,
        };
        let program = ocean_gpu::compile(&src, &stream).map_err(|why| {
            eprintln!("native-ocean: the ocean pass does not compile for stream type {kind} ({why})");
            "program"
        })?;
        let program = Arc::new(program);
        *cached = Cached { program: Some((world, kind, Arc::clone(&program))), grid: None };
        Ok(program)
    }

    fn grid(&self, program: &OceanProgram, mem: &dyn GuestMemory, stream: &ocean::Stream, quads: &[[u16; 4]]) -> Result<Arc<OceanGrid>, ocean::Cause> {
        let vertices = stream.vertices(mem)?;
        let key = grid_key(vertices, quads);
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(grid) = cached.grid.as_ref().filter(|g| g.key == key) {
            return Ok(Arc::clone(grid));
        }
        let len = ocean_gpu::vertex_count(quads).checked_mul(program.stride).ok_or("stream")?;
        let bytes = guest_bytes(mem, vertices, len).ok_or("stream")?;
        let grid = Arc::new(ocean_gpu::grid(program, &bytes, quads, key).map_err(|_| "stream")?);
        cached.grid = Some(Arc::clone(&grid));
        Ok(grid)
    }
}

impl water_end::OceanHost for Glue {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: &ocean::Draw, quads: &frame_scene::late::WaterComposite) -> Result<(), ocean::Cause> {
        let ready = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(current_ready(&state, &*mem).ok_or("world")?)
        };
        let data = ready.ocean.get().ok_or("world")?;
        let data = data.as_ref().as_ref().map_err(|_| "program")?;
        if data.shader.0 & 0xffff != draw.ocean.shader & 0xffff {
            return Err("program");
        }
        let program = self.program(&ready, data, &*mem, draw.stream.kind)?;
        let grid = self.grid(&program, &*mem, &draw.stream, &quads.quads)?;
        // The reflection and refraction targets (0x15, 0x16): the frame
        // graph's surfaces for their roles, which the copy and the
        // reflection drew into before this draw.
        let (targets, target_sizes) = sampled_targets(&*mem, &program.sampled())?;
        if data.palettes.iter().any(Option::is_some) {
            unimplemented!("ocean: stage palettes from the device");
        }
        let pass = OceanPass {
            data: Arc::clone(&ready),
            program,
            grid,
            rows: draw.ocean.rows.map(|r| r.map(f32::from_bits)),
            structure: draw.ocean.structure,
            time: native_render::layer_pass::render_time(&ReadMemory(&*mem)),
            targets,
            target_sizes,
        };
        crate::native_targets_glue::push(&*mem, Slot::AfterLights, pass);
        Ok(())
    }
}

/// The frame targets of the game targets `targets` (in order) and their
/// guest sizes: each a live table entry's first surface
/// (`native_shadows_glue::target_header`) named by its role, else
/// `target`.
fn sampled_targets(mem: &dyn GuestMemory, targets: &[u8]) -> Result<(Vec<TargetRef>, Vec<(u32, u32)>), ocean::Cause> {
    targets
        .iter()
        .map(|&t| {
            let header = crate::native_shadows_glue::target_header(mem, u32::from(t)).ok_or("target")?;
            match crate::native_targets_glue::target_ref(mem, header, true) {
                TargetRef::Game(g) => Ok((TargetRef::Game(g), g.size)),
                _ => Err("target"),
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.into_iter().unzip())
}

/// One ocean draw as the render thread records it.
struct OceanPass {
    /// Keeps the world (and so its ocean data) alive while queued.
    data: Arc<Ready>,
    program: Arc<OceanProgram>,
    grid: Arc<OceanGrid>,
    rows: [[f32; 4]; 3],
    structure: frame_scene::StructureParameters,
    time: Option<f64>,
    /// The targets the pass samples, in `OceanProgram::sampled`'s order,
    /// and their guest sizes.
    targets: Vec<TargetRef>,
    target_sizes: Vec<(u32, u32)>,
}

impl native_render::Stage for OceanPass {
    fn name(&self) -> &'static str {
        "ocean"
    }

    fn samples(&self) -> Vec<TargetRef> {
        self.targets.clone()
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        let Some(w_clip) = view.w_clip else { unimplemented!("ocean: the view's W-buffer clip") };
        let (data, program, grid) = (Arc::clone(&self.data), Arc::clone(&self.program), Arc::clone(&self.grid));
        let (rows, structure, time) = (self.rows, self.structure, self.time);
        let (device, queue, uploads) = (t.device.clone(), t.queue.clone(), t.uploads.clone());
        let (color, depth, formats) = (t.color.clone(), t.depth.clone(), (t.color_format, t.depth_format));
        let (guest_size, size, z_scale, constants) = (view.guest_size, view.size, view.z_scale, view.constants);
        let (sampled, target_sizes) = (t.sampled.clone(), self.target_sizes.clone());
        crate::native_targets_glue::encoded(move |encoder| {
            let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
            if gpu.as_ref().is_none_or(|(d, set)| *d != device || !set.holds(&program)) {
                let images: &[(Stage, Image)] = data.ocean.get().and_then(|d| d.as_ref().as_ref().ok()).map_or(&[], |d| &d.images);
                let mut stage_image = |s: Stage| images.iter().find(|(k, _)| *k == s).map(|(_, i)| i.clone());
                match OceanSet::new(&device, &queue, Arc::clone(&program), &mut stage_image) {
                    Ok(set) => *gpu = Some((device.clone(), set)),
                    Err(why) => {
                        eprintln!("native-ocean: the ocean set does not upload ({why})");
                        water_end::ocean_unported("program");
                    }
                }
            }
            let Some((_, set)) = gpu.as_mut() else { return };
            let draw = OceanDraw {
                view: constants,
                w_clip: Some(w_clip),
                rows,
                structure: &structure,
                time,
                grid: &grid,
                target_sizes: &target_sizes,
            };
            let mut target = OceanTarget {
                device: &device,
                queue: &queue,
                uploads: &uploads,
                encoder,
                color: &color,
                color_format: formats.0,
                depth: &depth,
                depth_format: formats.1,
                viewport: [0.0, 0.0, size.0 as f32, size.1 as f32],
                guest_size,
                size,
                z_scale,
                sampled: &sampled,
            };
            if let Err(cause) = set.record(&mut target, &draw) {
                water_end::ocean_unported(cause);
            }
        })
    }
}

/// Installs the ocean host over the world's state
/// (`native_world_glue::install`).
pub fn install(state: State) {
    water_end::set_ocean_host(Arc::new(Glue { state, cached: Mutex::new(Cached::default()) }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn guest_bytes_reads_a_tail_shorter_than_a_word() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        for i in 0..8u32 {
            m.set_u8(0x1010 + i, i as u8 + 1).unwrap();
        }
        assert_eq!(guest_bytes(&m, 0x1010, 6).unwrap(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(guest_bytes(&m, 0x1010, 0).unwrap(), Vec::<u8>::new());
        assert!(guest_bytes(&m, 0x10fe, 8).is_none(), "past memory");
    }

    /// Target `n` live, its first surface linear A8R8G8B8 of `w` x `h`.
    fn live_target(m: &mut FlatMemory, n: u32, (w, h): (u32, u32)) {
        use render_bridge::native_targets::{TARGET_STRIDE, TARGET_SURFACE, TARGETS};
        let d = TARGETS + n * TARGET_STRIDE;
        m.set_u32(d + 0x88, 0x1000).unwrap();
        m.set_u32(d + 0x78, 1).unwrap();
        let header = d + TARGET_SURFACE;
        m.set_u32(header + 0x0c, (0x12 << 8) | (2 << 4)).unwrap();
        m.set_u32(header + 0x10, (w - 1) | ((h - 1) << 12) | ((w * 4 / 64 - 1) << 24)).unwrap();
    }

    #[test]
    fn the_reflection_and_refraction_are_their_roles_frame_targets() {
        use frame_types::{GameTarget, TargetRole};
        use render_bridge::native_targets::{TARGET_STRIDE, TARGETS};
        let mut m = FlatMemory::new(TARGETS, (0x20 * TARGET_STRIDE) as usize);
        live_target(&mut m, 0x16, (640, 480));
        live_target(&mut m, 0x15, (512, 256));
        let game = |n, size| TargetRef::Game(GameTarget { role: TargetRole::Game(n), size, depth: false });
        let (refs, sizes) = sampled_targets(&m, &[0x16, 0x15]).unwrap();
        assert_eq!(refs, [game(0x16, (640, 480)), game(0x15, (512, 256))]);
        assert_eq!(sizes, [(640, 480), (512, 256)]);
        assert_eq!(sampled_targets(&m, &[]).unwrap(), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_target_not_made_is_the_target_cause() {
        use render_bridge::native_targets::{TARGET_STRIDE, TARGETS};
        let mut m = FlatMemory::new(TARGETS, (0x20 * TARGET_STRIDE) as usize);
        live_target(&mut m, 0x16, (640, 480));
        assert_eq!(sampled_targets(&m, &[0x16, 0x15]).unwrap_err(), "target");
        m.set_u8(TARGETS + 0x16 * TARGET_STRIDE + 0x95, 1).unwrap();
        assert_eq!(sampled_targets(&m, &[0x16]).unwrap_err(), "target", "disabled");
    }

    #[test]
    fn a_grid_key_follows_the_stream_and_the_quads() {
        let quads = [[0, 1, 2, 3]];
        assert_eq!(grid_key(0x8000_1000, &quads), grid_key(0x8000_1000, &quads));
        assert_ne!(grid_key(0x8000_1000, &quads), grid_key(0x8000_2000, &quads));
        assert_ne!(grid_key(0x8000_1000, &quads), grid_key(0x8000_1000, &[[0, 1, 3, 2]]));
    }
}
