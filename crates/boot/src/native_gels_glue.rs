//! Target-13 producer host. The first slice observes one complete guest
//! quad, then produces the native equivalent in render order. It drops
//! no D3D work. Lights receive an invocation-specific ticket through
//! `Outputs`; no process-global last-texture slot is used.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use feature::native_gels::{Call, Host};
use gfx::state::{DepthBufferMode, FillMode, ShadeMode, TextureAddress};
use gfx::translate::{Capabilities, SamplerKey};
use native_render::camera::GuestRead;
use native_render::gels::{self, Key, Target};
use native_render::gels_gpu::{self, Frame, Producer, Program, Ticket};
use native_render::pass_state::PassState;
use shaders::psh::{PixelOptions, PixelUniforms, TextureDim};
use shaders::vsh::VertexUniforms;

use crate::native_gels_load::{self, Asset, Assets};
use crate::native_lights_glue::DeviceMemory;

struct Ro<'a>(&'a dyn GuestMemory);
impl GuestRead for Ro<'_> {
    fn read_u32(&self, address: u32) -> Option<u32> {
        self.0.u32(address).ok()
    }
}

/// Pass this owner to the light graph when installing the host. Retain
/// the returned ticket for the exact light, then ask it for the current
/// copy inside encode. An unproduced copy always returns None.
#[derive(Default)]
pub struct Outputs {
    latest: Mutex<Option<Ticket>>,
    sequence: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
pub struct Checkpoint(u64);

impl Outputs {
    /// Take at the light's entry, before its constants run. A whole-light
    /// skip then cannot accidentally consume the previous light's gel.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.sequence.load(Ordering::Acquire))
    }

    pub fn produced_after(
        &self,
        before: Checkpoint,
        definition: u32,
        shader: u32,
        surface: u32,
    ) -> Option<Ticket> {
        let ticket = self.latest.lock().ok()?.clone()?;
        let key = ticket.key;
        (key.sequence > before.0
            && key.definition == definition
            && key.shader == shader
            && key.surface == surface)
            .then_some(ticket)
    }
}

#[derive(Default)]
struct Loaded {
    name: String,
    generation: u64,
    assets: Option<Arc<Assets>>,
}

struct Pending {
    call: Call,
    name: String,
    key: Key,
}

struct Glue<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    loaded: Arc<Mutex<Loaded>>,
    pending: Mutex<Option<Pending>>,
    outputs: Arc<Outputs>,
    compares: AtomicU64,
    refused: Arc<AtomicU64>,
    encoded: Arc<AtomicU64>,
}

fn compare_budget() -> u64 {
    static BUDGET: OnceLock<u64> = OnceLock::new();
    *BUDGET.get_or_init(|| match std::env::var("H2_NATIVE_GELS_COMPARE") {
        Err(_) => 0,
        Ok(s) if s.is_empty() => 4,
        Ok(s) => s.parse::<u64>().unwrap_or_else(|_| {
            eprintln!("native-gels: H2_NATIVE_GELS_COMPARE must be a count; compare disabled");
            0
        }),
    })
}

impl<B: d3d::Backend + Send + 'static> Glue<B> {
    fn refuse(&self, why: &str) {
        let n = self.refused.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_multiple_of(1800) {
            eprintln!("native-gels: production refused ({n}): {why}; D3D retained");
        }
    }

    fn assets(&self, name: &str) -> Option<Arc<Assets>> {
        let mut loaded = self.loaded.lock().ok()?;
        if loaded.name == name {
            return loaded.assets.clone();
        }
        loaded.name = name.into();
        loaded.generation = loaded.generation.wrapping_add(1);
        loaded.assets = None;
        let generation = loaded.generation;
        let slot = Arc::clone(&self.loaded);
        let name = name.to_owned();
        drop(loaded);
        let _ = std::thread::Builder::new()
            .name("native gels load".into())
            .spawn(move || match native_gels_load::load(&name) {
                Ok(assets) => {
                    let accepted = assets.values().filter(|v| v.is_ok()).count();
                    eprintln!(
                        "native-gels: {name}: {accepted} of {} shader gels loaded",
                        assets.len()
                    );
                    let mut loaded = slot.lock().unwrap_or_else(|e| e.into_inner());
                    if loaded.name == name && loaded.generation == generation {
                        loaded.assets = Some(Arc::new(assets));
                    }
                }
                Err(e) => eprintln!("native-gels: {name}: load refused: {e}"),
            });
        None
    }

    fn prepare(
        &self,
        mem: &mut dyn GuestMemory,
        mxcsr: u32,
        pending: Pending,
    ) -> Result<(), String> {
        if crate::health::loaded_map_name(mem).map_err(|e| format!("map name: {e:?}"))?
            != pending.name
        {
            return Err("map changed during gel setup".into());
        }
        if mem.u32(gels::DEFINITION).ok() != Some(pending.call.definition)
            || mem.u32(pending.call.definition.wrapping_add(0xe0)).ok() != Some(pending.call.shader)
        {
            return Err("light changed during gel setup".into());
        }
        let assets = self
            .assets(&pending.name)
            .ok_or("gel assets are not resident")?;
        let gel = mem
            .u32(
                pending
                    .call
                    .definition
                    .checked_add(0x7c)
                    .ok_or("definition overflow")?,
            )
            .map_err(|e| format!("gel datum: {e:?}"))?;
        let asset = Arc::clone(
            assets
                .get(&(pending.call.shader, gel))
                .ok_or("gel shader not loaded")?
                .as_ref()
                .map_err(Clone::clone)?,
        );
        if asset.uses_light_bitmap
            && (mem
                .u16(engine::light_constants::global::CONTEXT_WORD)
                .ok()
                .is_none_or(|w| w == 0)
                || mem
                    .u32(pending.call.definition)
                    .ok()
                    .is_none_or(|f| f & engine::light_passes::DEFINITION_TEXTURE_18 != 0))
        {
            return Err("gel engine texture 10 is redirected".into());
        }
        let target =
            Target::read(&Ro(mem)).ok_or("target 13 is not a resident single-level target")?;
        if target.surface != pending.key.surface {
            return Err("target 13 changed during setup".into());
        }
        let depth = engine::gel::depth(
            mem.u32(engine::gel::NEAR)
                .map_err(|e| format!("near: {e:?}"))?,
            mem.u32(engine::gel::FAR)
                .map_err(|e| format!("far: {e:?}"))?,
            mem.u32(engine::gel::ONE)
                .map_err(|e| format!("one: {e:?}"))?,
            mem.u32(engine::gel::DEPTH_SCALE)
                .map_err(|e| format!("depth scale: {e:?}"))?,
            mxcsr,
        )
        .ok_or("quad depth raises an unmasked exception")?;
        let ticket = Ticket::new(pending.key);
        let frame = Arc::new(Mutex::new(None));
        let mut device = self.device.lock().map_err(|_| "device lock")?;
        if !device.can_external_pass() || device.swallowing() {
            return Err("external observer unavailable or another observer active".into());
        }
        let pass = Pass {
            asset: Arc::clone(&asset),
            frame: Arc::clone(&frame),
            ticket: ticket.clone(),
            target,
            compare: self.compares.fetch_add(1, Ordering::Relaxed) < compare_budget(),
            encoded: Arc::clone(&self.encoded),
            refused: Arc::clone(&self.refused),
            clip_known: false,
        };
        // This drains guest setup before constants are copied. drop=false
        // is mandatory: an observer mismatch cannot lose a guest draw.
        let armed = device
            .swallow(
                &mut DeviceMemory(mem),
                d3d::Swallow {
                    targets: vec![1],
                    primitive: gfx::vertex::PrimitiveType::TriangleStrip,
                    vertices: 4,
                    shapes: Vec::new(),
                    drop: false,
                    pass: Box::new(pass),
                },
            )
            .map_err(|e| e.to_string())?;
        if !armed {
            return Err("observer not admitted".into());
        }
        let state = snapshot(&device, mem, &asset, target, depth)?;
        *frame.lock().map_err(|_| "gel frame lock")? = Some(state);
        *self.outputs.latest.lock().map_err(|_| "gel output lock")? = Some(ticket);
        Ok(())
    }
}

impl<B: d3d::Backend + Send + 'static> Host for Glue<B> {
    fn template(&self, mem: &dyn GuestMemory, call: Option<Call>) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        self.outputs
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(call) = call else { return };
        let Some(target) = Target::read(&Ro(mem)) else {
            self.refuse("target 13 is unresolved or has mip levels");
            return;
        };
        let Ok(name) = crate::health::loaded_map_name(mem) else {
            return;
        };
        self.assets(&name);
        let key = Key {
            sequence: self.outputs.sequence.fetch_add(1, Ordering::AcqRel) + 1,
            definition: call.definition,
            shader: call.shader,
            surface: target.surface,
        };
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { call, name, key });
    }

    fn quad(&self, mem: &mut dyn GuestMemory, mxcsr: u32) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(pending) = pending
            && let Err(e) = self.prepare(mem, mxcsr, pending)
        {
            self.refuse(&e);
        }
    }
}

struct Snapshot {
    program: Program,
    frame: Frame,
}

fn snapshot<B: d3d::Backend>(
    device: &d3d::Device<B>,
    mem: &mut dyn GuestMemory,
    asset: &Asset,
    target: Target,
    depth: [u32; 2],
) -> Result<Snapshot, String> {
    let raw = d3d::state::StateWords::read(&DeviceMemory(mem), &device.config().layout)
        .map_err(|e| e.to_string())?;
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    let state = raw.translate(&table);
    if !state.bad.is_empty() {
        return Err(format!("invalid gel state: {:?}", state.bad));
    }
    let rs = &state.render;
    if rs.depth_mode != DepthBufferMode::Disabled
        || rs.stencil.enable
        || rs.fog_enable
        || rs.alpha_test_enable
        || rs.shade_mode != ShadeMode::Gouraud
        || rs.fill_mode != FillMode::Solid
        || rs.cull_mode != gfx::state::CullMode::None
    {
        return Err("gel requires unsupported depth/stencil/fog/alpha/shading state".into());
    }
    let extras = raw.stage_extras();
    if extras
        .iter()
        .any(|e| e.color_key_op != 0 || e.color_sign != 0)
    {
        return Err("gel needs colour keying or signed textures".into());
    }
    if state
        .stages
        .iter()
        .any(|s| [s.address_u, s.address_v, s.address_w].contains(&TextureAddress::Border))
    {
        return Err("gel needs border sampling".into());
    }
    // The combiner bank is the evaluated one, including animation and
    // engine constants. Only a non-variant implementation is admitted.
    let mut words = [0; 60];
    for (word, bytes) in words.iter_mut().zip(asset.pixel_shader.chunks_exact(4)) {
        *word = u32::from_le_bytes(bytes.try_into().unwrap());
    }
    let final_latch = device
        .config()
        .layout
        .device
        .checked_add(0x788)
        .and_then(|a| mem.u32(a).ok())
        .ok_or("final combiner latch")?
        != 0;
    let def = d3d::nv2a::guest_def(
        &words,
        raw.render[..57].try_into().unwrap(),
        raw.render[d3d::state::slot::PS_TEXTURE_MODES as usize],
        final_latch,
    );
    if !def.has_final_combiner() {
        return Err("gel needs the implicit final combiner".into());
    }
    let raw_state = |canonical| {
        table
            .canonical_to_guest(canonical)
            .map(|n| raw.render[n as usize])
            .unwrap_or(0)
    };
    if raw_state(141) != 0 || device.hw().stipple_control.unwrap_or_else(|| raw_state(83)) != 0 {
        return Err("gel needs two-sided lighting or polygon stipple".into());
    }
    let options = PixelOptions {
        texture_dims: asset
            .images
            .each_ref()
            .map(|i| i.as_ref().map(|_| TextureDim::D2)),
        alpha_kill: extras.map(|e| e.alpha_kill & 4 != 0),
        ..Default::default()
    };
    let caps = Capabilities::from_features(wgpu::Features::empty());
    let samplers = std::array::from_fn(|n| SamplerKey::from_stage(&state.stages[n], caps));
    let pass_state = PassState {
        blend: gfx::translate::blend_state(rs),
        blend_constant: gfx::translate::blend_constant(rs),
        color_writes: gfx::translate::color_writes(rs.color_write),
        depth_write: false,
        depth_compare: wgpu::CompareFunction::Always,
        primitive: gfx::translate::primitive_state(
            rs,
            wgpu::PrimitiveTopology::TriangleStrip,
            caps,
        ),
        alpha_test: None,
    };
    let program = Program::compile(&asset.code, &def, &options, samplers, pass_state)?;
    let mut vertex = VertexUniforms {
        constants: std::array::from_fn(|r| device.constant(r)),
        z_max: gfx::renderer::d3dz_max(None),
        w_clip: None,
        ..Default::default()
    };
    vertex.surface = [target.size.0 as f32, target.size.1 as f32];
    if device
        .hw()
        .control0
        .is_some_and(|c| c & d3d::nv2a::CONTROL0_Z_PERSPECTIVE != 0)
    {
        vertex.w_clip = Some(d3d::nv2a::w_clip_range(device.hw(), vertex.z_max));
        vertex.w_clamp = d3d::nv2a::clamps_depth(raw_state(d3d::nv2a::RS_DEPTHCLIPCONTROL));
    }
    let (c0, c1) = def.embedded_constants();
    let [x, y, z] = device.hw().eye_vector;
    let mut pixel = PixelUniforms {
        c0,
        c1,
        eye: [x, y, z, 0.0],
        ..Default::default()
    };
    for n in 0..4 {
        let e = extras[n];
        pixel.bump_mat[n] = e.bump_env_mat;
        pixel.bump_lum[n] = [e.bump_env_lscale, e.bump_env_loffset, 0.0, 0.0];
        let bias = state.stages[n].mip_lod_bias;
        pixel.lod_bias[n] = if bias.is_finite() { bias } else { 0.0 };
    }
    Ok(Snapshot {
        program,
        frame: Frame {
            vertex,
            pixel,
            vertices: gels_gpu::quad(target.size, depth.map(f32::from_bits)),
        },
    })
}

struct Pass {
    asset: Arc<Asset>,
    frame: Arc<Mutex<Option<Snapshot>>>,
    ticket: Ticket,
    target: Target,
    compare: bool,
    encoded: Arc<AtomicU64>,
    refused: Arc<AtomicU64>,
    clip_known: bool,
}
impl d3d::ExternalPass for Pass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        if let Ok(mut frame) = self.frame.lock()
            && let Some(snapshot) = frame.as_mut()
        {
            snapshot.frame.vertex.constants[..d3d::VIEW_CONSTANTS].copy_from_slice(constants);
        }
    }
    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip_known = clip.w_buffered.is_some();
        if let Ok(mut frame) = self.frame.lock()
            && let Some(snapshot) = frame.as_mut()
        {
            snapshot.frame.vertex.w_clip = (clip.w_buffered == Some(true)).then_some(clip.range);
            snapshot.frame.vertex.w_clamp = clip.clamp;
        }
    }
    fn colour_only(&self) -> bool {
        true
    }
    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let result = (|| {
            if !self.clip_known {
                return Err("gel W/Z mode is unknown".into());
            }
            if t.guest_size != self.target.size
                || t.viewport != [0.0, 0.0, t.size.0 as f32, t.size.1 as f32]
            {
                return Err("gel copy has another viewport or size".into());
            }
            let frame = self.frame.lock().map_err(|_| "gel frame lock")?;
            let snapshot = frame.as_ref().ok_or("gel snapshot not complete")?;
            gels_gpu::full_target(&self.asset.code, &snapshot.frame, t.guest_size)?;
            // Program is pure but not Clone; recreate from its immutable
            // source/state for each copy without sharing mutable buffers.
            let program = Program {
                wgsl: snapshot.program.wgsl.clone(),
                dims: snapshot.program.dims,
                samplers: snapshot.program.samplers,
                state: snapshot.program.state,
            };
            let producer = Producer::new(t.device, t.queue, program, &self.asset.images)?;
            let output = producer.encode(
                t.device,
                t.encoder,
                self.ticket.key,
                &snapshot.frame,
                t.color_format,
                t.guest_size,
                t.size,
            )?;
            if self.compare {
                gels_gpu::compare_on_submit(t.device, t.encoder, &output, t.color)?;
            }
            self.ticket.publish(output);
            let count = self.encoded.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= 4 || count.is_multiple_of(1800) {
                eprintln!(
                    "native-gels: produced {count} target-13 copies, {:?} {:?}",
                    self.ticket.key, t.size
                );
            }
            Ok::<(), String>(())
        })();
        if let Err(e) = result {
            let count = self.refused.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= 8 || count.is_multiple_of(1800) {
                eprintln!(
                    "native-gels: {:?}: encode refused ({count}): {e}; D3D retained",
                    self.ticket.key
                );
            }
        }
    }
}

/// Installed with the world's hosts. The lights lane can retain the
/// returned Outputs and bind its tickets at the post-constants seam.
pub fn install<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) -> Arc<Outputs> {
    let outputs = Arc::new(Outputs::default());
    feature::native_gels::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        loaded: Arc::new(Mutex::new(Loaded::default())),
        pending: Mutex::new(None),
        outputs: Arc::clone(&outputs),
        compares: AtomicU64::new(0),
        refused: Arc::new(AtomicU64::new(0)),
        encoded: Arc::new(AtomicU64::new(0)),
    }));
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_light_without_new_production_cannot_take_the_previous_lights_gel() {
        let outputs = Outputs::default();
        let before_first = outputs.checkpoint();
        let key = Key {
            sequence: 1,
            definition: 2,
            shader: 3,
            surface: 4,
        };
        outputs.sequence.store(1, Ordering::Release);
        *outputs.latest.lock().unwrap() = Some(Ticket::new(key));
        assert!(outputs.produced_after(before_first, 2, 3, 4).is_some());
        let before_second = outputs.checkpoint();
        assert!(outputs.produced_after(before_second, 2, 3, 4).is_none());
        assert!(outputs.produced_after(before_first, 2, 3, 5).is_none());
        let next = Key { sequence: 2, ..key };
        outputs.sequence.store(2, Ordering::Release);
        *outputs.latest.lock().unwrap() = Some(Ticket::new(next));
        assert_eq!(
            outputs.produced_after(before_second, 2, 3, 4).unwrap().key,
            next
        );
    }
}
