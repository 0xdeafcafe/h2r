//! Native decal admission: ordered D3D state, the engine's vertex program,
//! live bitmap selection/residency, and one external pass per `0x00044040`
//! callback. Inert unless `impl.render.native_decals` or its compare variable
//! is on. Any unresolved item refuses the whole callback and leaves D3D to it.
//!
//! Images are decoded from the live resident resource selected by each record,
//! not from a map cache. That proves the sampled header, format, mip chain and
//! bytes all belong to the same draw snapshot.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use engine::memory::GuestMemory;
use feature::native_decals::Host;
use gfx::texture::{TextureDesc, TextureKind};
use native_render::decal::{self, Memory, Snapshot};
use native_render::decal_gpu::{
    DecalClip, DecalGpu, DecalImage, DecalTarget, Prepared, ShaderSnapshot, StageExtras,
    view_inputs,
};

use crate::native_world_glue::ReadMemory;

struct Ro<'a>(&'a dyn GuestMemory);

impl Memory for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }

    fn read_u16(&self, addr: u32) -> Option<u16> {
        self.0.u16(addr).ok()
    }

    fn read_bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.0.bytes(addr, len).ok()
    }

    fn write_u16(&mut self, _addr: u32, _value: u16) {
        unreachable!("read-only snapshot adapter")
    }
}

struct Glue<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

/// Installs the host. No world/BSP loader is required.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    feature::native_decals::set_host(Arc::new(Glue {
        device: Arc::clone(device),
    }));
}

fn decode_live(
    mem: &dyn GuestMemory,
    pc: &d3d::resource::PixelContainer,
    desc: &TextureDesc,
) -> Result<DecalImage, &'static str> {
    use gfx::D3DFormat as F;
    if matches!(
        desc.format,
        F::P8
            | F::Dxt1
            | F::Dxt3
            | F::Dxt5
            | F::G8B8
            | F::LinG8B8
            | F::R8B8
            | F::LinR8B8
            | F::V16U16
            | F::LinV16U16
            | F::D24S8
            | F::F24S8
            | F::D16
            | F::F16
            | F::LinD24S8
            | F::LinF24S8
            | F::LinD16
            | F::LinF16
    ) {
        return Err("bitmap format");
    }
    let address = d3d::AddressMap::XboxContiguous.to_virtual(pc.resource.data);
    let bytes = mem
        .bytes(address, desc.total_size().map_err(|_| "texture size")?)
        .map_err(|_| "texture bytes")?;
    let decoded = gfx::decode_texture(
        desc,
        &bytes,
        None,
        gfx::DecodeOptions::default().with_color_sign(0),
    )
    .map_err(|_| "bitmap decode")?;
    let bgra = decoded.host == gfx::format::HostFormat::Bgra8Unorm;
    let levels: Vec<_> = decoded
        .levels
        .into_iter()
        .filter(|l| l.face == 0 && l.depth <= 1)
        .map(|mut l| {
            if bgra {
                for px in l.data.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
            }
            (l.width, l.height, l.data)
        })
        .collect();
    let image = native_render::gpu::Image::from_levels(levels).ok_or("bitmap levels")?;
    Ok(DecalImage {
        image,
        format: desc.format,
        linear: desc.format.layout() == gfx::format::Layout::Linear,
    })
}

fn resolve_images(
    mem: &dyn GuestMemory,
    snapshot: &mut Snapshot,
) -> Result<HashMap<(u32, u32), DecalImage>, &'static str> {
    let generation = mem
        .u32(decal::BITMAP_GENERATION)
        .map_err(|_| "bitmap residency")?;
    let mut images = HashMap::new();
    let mut headers = HashMap::new();
    for d in &mut snapshot.draws {
        let (index, guest) =
            decal::selected_image(&Ro(mem), d.bitmap, d.selector).ok_or("default bitmap")?;
        let resident = mem.u32(guest + 0x70).map_err(|_| "bitmap residency")? > generation;
        let header = mem.u32(guest + 0x50).map_err(|_| "bitmap residency")?;
        if !resident || header == 0 {
            return Err("bitmap residency");
        }
        if headers
            .insert((d.bitmap, index), header)
            .is_some_and(|old| old != header)
        {
            return Err("texture identity");
        }
        let pc = d3d::resource::PixelContainer::read(&ReadMemory(mem), header)
            .map_err(|_| "texture header")?;
        if pc.resource.kind() != d3d::resource::ResourceType::Texture {
            return Err("texture header");
        }
        let desc = TextureDesc::from_header(pc.format, pc.size).map_err(|_| "texture header")?;
        if desc.kind != TextureKind::D2 {
            return Err("texture kind");
        }
        let image = match images.entry((d.bitmap, index)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(decode_live(mem, &pc, &desc)?),
        };
        if (desc.width, desc.height, desc.levels)
            != (
                image.image.width,
                image.image.height,
                1 + image.image.mips.len() as u32,
            )
            || (desc.format.layout() == gfx::format::Layout::Linear) != image.linear
            || desc.format != image.format
        {
            return Err("resident texture");
        }
        d.image = Some(index);
        d.texture_header = Some(header);
    }
    Ok(images)
}

fn prepare(mem: &dyn GuestMemory, mut snapshot: Snapshot) -> Result<Prepared, &'static str> {
    let words = d3d::state::StateWords::read(&ReadMemory(mem), &d3d::GuestLayout::HALO2)
        .map_err(|_| "state read")?;
    let state = words.translate(&gfx::xdk::RenderStateTable::default());
    if !state.bad.is_empty() {
        return Err("state decode");
    }
    let extras = words.stage_extras().map(|e| StageExtras {
        color_key_op: e.color_key_op,
        color_sign: e.color_sign,
        alpha_kill: e.alpha_kill,
        texture_transform_flags: e.texture_transform_flags,
        bump_env_mat: e.bump_env_mat,
        bump_env_lscale: e.bump_env_lscale,
        bump_env_loffset: e.bump_env_loffset,
    });
    native_render::decal_gpu::check_state(snapshot.mode, &state.render, &state.stages, &extras)?;
    if words.render[83] != 0 || words.render[141] != 0 {
        return Err("lighting/stipple");
    }
    let psdef =
        d3d::guest::read_array::<60>(&ReadMemory(mem), 0x0048_4f68).map_err(|_| "combiner")?;
    let final_latch = mem
        .u32(d3d::GuestLayout::HALO2.device + 0x788)
        .map_err(|_| "final combiner")?
        != 0;
    let combiner_state: &[u32; 57] = words.render[..57].try_into().map_err(|_| "combiner")?;
    let combiner = d3d::nv2a::guest_def(&psdef, combiner_state, words.render[136], final_latch);
    let images = resolve_images(mem, &mut snapshot)?;
    let (microcode, _) = decal::vertex_program(&Ro(mem)).ok_or("vertex program")?;
    let prepared = Prepared {
        snapshot,
        shader: ShaderSnapshot {
            render: state.render,
            stages: state.stages,
            extras,
            combiner,
            microcode,
            images,
            integer_depth: true,
        },
    };
    native_render::decal_gpu::check_program(&prepared.snapshot, &prepared.shader)?;
    Ok(prepared)
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn admit(
        &self,
        mem: &dyn GuestMemory,
        snapshot: Snapshot,
        draw: bool,
    ) -> Result<(), &'static str> {
        let mut prepared = prepare(mem, snapshot)?;
        let Ok(mut d) = self.device.lock() else {
            return Err("device");
        };
        let depth = d.bound_target()[1];
        if depth == 0 {
            if prepared.snapshot.mode.w_buffered() {
                return Err("depth target");
            }
        } else {
            let pc = d3d::resource::PixelContainer::read(&ReadMemory(mem), depth)
                .map_err(|_| "depth target")?;
            let desc = TextureDesc::from_header(pc.format, pc.size).map_err(|_| "depth target")?;
            prepared.shader.integer_depth = !matches!(
                desc.format,
                gfx::D3DFormat::F16
                    | gfx::D3DFormat::LinF16
                    | gfx::D3DFormat::F24S8
                    | gfx::D3DFormat::LinF24S8
            );
        }
        if !draw {
            return Ok(());
        }
        if !d.can_external_pass() {
            return Err("external pass");
        }
        d.request_external_pass(Box::new(DecalPass {
            prepared,
            constants: None,
            clip: None,
            gpu: None,
        }));
        Ok(())
    }
}

struct DecalPass {
    prepared: Prepared,
    constants: Option<[[f32; 4]; d3d::VIEW_CONSTANTS]>,
    clip: Option<d3d::DepthClip>,
    gpu: Option<DecalGpu>,
}

impl d3d::ExternalPass for DecalPass {
    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.constants = Some(*constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn encode(&mut self, mut t: d3d::ExternalTarget<'_>) {
        let (Some(constants), Some(clip)) = (self.constants, self.clip) else {
            feature::native_decals::note_refusal("issue snapshot");
            return;
        };
        if self.gpu.is_none() {
            match DecalGpu::new(t.device, t.queue, &self.prepared) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(why) => {
                    feature::native_decals::note_refusal(why);
                    return;
                }
            }
        }
        let clip = DecalClip {
            range: clip.range,
            clamp: clip.clamp,
            w_buffered: clip.w_buffered,
        };
        let mut target = DecalTarget {
            device: t.device,
            queue: t.queue,
            encoder: &mut *t.encoder,
            color: t.color,
            color_format: t.color_format,
            depth: t.depth,
            depth_format: t.depth_format,
            viewport: t.viewport,
            z_scale: t.z_scale,
            guest_size: t.guest_size,
            size: t.size,
        };
        let view = view_inputs(&target, &constants, clip);
        if !self
            .gpu
            .as_mut()
            .expect("created above")
            .encode(&mut target, &view, clip)
        {
            feature::native_decals::note_refusal("encode");
        }
    }
}

/// The distinct images a callback resolves, in deterministic load order.
pub fn required_images(snapshot: &Snapshot) -> BTreeSet<(u32, i8)> {
    snapshot
        .draws
        .iter()
        .map(|d| (d.bitmap, d.selector))
        .collect()
}
