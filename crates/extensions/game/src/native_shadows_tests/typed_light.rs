//! A diffuse/specular light inserted between the shadow receivers and a D3D
//! gel. Both groups cover the scissor. Diffuse writes green and alpha,
//! specular overwrites alpha, and the following gel overwrites part of it.
use super::*;
use d3d::{
    Backend, ExternalPassRequest, ExternalPlan, ExternalSurfaces, GfxBackend, TargetCopy,
    backend::{ExternalAttachments, ExternalDraws},
};
use frame_scene::{GroupSite, LightConstants, LightGroup};
use gfx::surface::SurfaceId;
use native_render::{
    light_gpu::{LightDraw, LightGpu, LightSet, LightTarget as GpuTarget},
    light_look::{LightProgram, LightStage},
    program::ViewInputs,
    world::WorldVertex,
};
use wgpu::util::DeviceExt;

const SCISSOR: [u32; 4] = [1, 1, 6, 2];

/// D3D's independent input is the ordinary one-texel colour texture in
/// the fixture. The typed path uses the resident light program and buffers.
pub(super) fn colour_draw() -> ResolvedDraw {
    let mut draw = receiver();
    draw.texture_info[0].as_mut().unwrap().format = D3DFormat::LinA8R8G8B8;
    let modes = texture_modes([TEX_PROJECT2D, 0, 0, 0]);
    Arc::make_mut(&mut draw.raw).render[136] = modes;
    Arc::make_mut(draw.shader.pixel_shader.as_mut().unwrap())[54] = modes;
    draw
}

pub(super) fn original(layer: u8) -> ResolvedDraw {
    let mut draw = colour_draw();
    let [x, y, width, height] = SCISSOR;
    draw.clip = Some(d3d::PixelRect {
        x,
        y,
        width,
        height,
    });
    draw.state.render.stencil.enable = false;
    draw.state.render.color_write.green = layer == 12;
    draw
}

fn program(layer: u8) -> LightProgram {
    let mut pixel = shaders::psh::PixelUniforms::default();
    pixel.c0[0] = if layer == 12 {
        [0.0, 1.0, 0.0, 1.0]
    } else {
        [0.0; 4]
    };
    LightProgram {
        wgsl: "
            struct Inputs { colour: vec4f }
            @group(0) @binding(1) var<uniform> inputs: Inputs;
            @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
                return vec4f(uv * 2.0 - 1.0, 0.5, 1.0);
            }
            @fragment fn fs_main() -> @location(0) vec4f { return inputs.colour; }
        "
        .into(),
        words: vec![],
        inputs: 0,
        def: Default::default(),
        pixel_options: Default::default(),
        stages: [LightStage::Empty; 4],
        samplers: [None; 4],
        dims: [None; 4],
        signed: [[false; 4]; 4],
        vertex: Default::default(),
        pixel,
        colours: vec![],
        state: native_render::pass_state::PassState {
            blend: None,
            blend_constant: wgpu::Color::BLACK,
            color_writes: if layer == 12 {
                wgpu::ColorWrites::GREEN | wgpu::ColorWrites::ALPHA
            } else {
                wgpu::ColorWrites::ALPHA
            },
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
            primitive: Default::default(),
            alpha_test: None,
            stencil: None,
            depth_offset: None,
        },
        inherits_cull: false,
        copies_target: false,
        animation: None,
    }
}

struct Group {
    program: Arc<LightProgram>,
    gpu: Mutex<LightGpu>,
    range: std::ops::Range<u32>,
}

struct PacketRun {
    set: LightSet,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    groups: Vec<Group>,
    scissor: [u32; 4],
    constants: LightConstants,
}

impl d3d::ExternalPass for PacketRun {
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let draws: Vec<_> = self
            .groups
            .iter()
            .map(|group| LightDraw {
                program: &group.program,
                gpu: &group.gpu,
                cull: None,
                sample: None,
                colours: [
                    self.constants.colour_a.map(f32::from_bits),
                    self.constants.colour_b.map(f32::from_bits),
                ],
                frame: &self.constants.frame,
                projection: None,
                gel: None,
                gel_view: None,
                gel_scale: None,
                shadow_map: None,
                ranges: std::slice::from_ref(&group.range),
                placement: None,
                model: None,
            })
            .collect();
        let target = GpuTarget {
            device: &t.device,
            queue: &t.queue,
            uploads: &t.uploads,
            color: &t.color,
            color_format: t.color_format,
            depth: &t.depth,
            depth_format: t.depth_format,
            size: t.size,
        };
        let view = ViewInputs {
            constants: [[0.0; 4]; d3d::VIEW_CONSTANTS],
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            w_clip: None,
        };
        Some(self.set.draws(
            &target,
            &self.vertices,
            &self.indices,
            &view,
            self.scissor,
            &draws,
        ))
    }
}

/// Both groups prepare in one packet run. In the refusal case only the
/// last group's indices are unavailable, so even diffuse stays with D3D.
pub(super) fn issue(
    backend: &mut GfxBackend,
    surfaces: &ExternalSurfaces<SurfaceId>,
    request: ExternalPassRequest,
    missing_specular_indices: bool,
) -> bool {
    let site = GroupSite::Light;
    let groups = [12, 13].map(|layer| LightGroup {
        light: 1,
        layer,
        site,
        projection: None,
        pass: None,
        entries: u32::from(layer - 12)..u32::from(layer - 11),
        remainder: 0,
    });
    let constants = LightConstants {
        flags: 3,
        frame: [0; 36],
        colour_a: [0; 3],
        colour_b: [0; 3],
        mode: 0,
        definition_flags: 0,
        definition_8a: 0,
        definition_80: 0,
        falloff_0: 0,
        gel: None,
        animated_gel: false,
    };
    let (device, queue) = (&backend.ctx.device, &backend.ctx.queue);
    let set = LightSet::new(device, queue, None, None);
    let color = backend.store.get(surfaces.color).unwrap().wgpu_format();
    let depth = backend.store.get(surfaces.depth).unwrap().wgpu_format();
    let mut held = Vec::new();
    for group in &groups {
        let layer = group.layer;
        let program = Arc::new(program(layer));
        let mut gpu = LightGpu::new(
            device,
            queue,
            program.clone(),
            set.uniforms_layout(),
            &mut |_, _| panic!("invented pass has no stage texture"),
        )
        .unwrap();
        assert!(
            gpu.pipeline(device, color, depth, None, None)
                .1
                .get()
                .is_some()
        );
        let missing = layer == 13 && missing_specular_indices;
        held.push(Group {
            program,
            gpu: Mutex::new(gpu),
            range: if missing { 3..6 } else { 0..3 },
        });
    }
    timeline::frame();
    let buffer = |usage, bytes: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytes,
            usage,
        })
    };
    let index_bytes: Vec<_> = [0u32, 1, 2]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let run = PacketRun {
        set,
        groups: held,
        constants,
        scissor: SCISSOR,
        vertices: buffer(
            wgpu::BufferUsages::VERTEX,
            &[0; 3 * std::mem::size_of::<WorldVertex>()],
        ),
        indices: buffer(wgpu::BufferUsages::INDEX, &index_bytes),
    };
    let plan = ExternalPlan {
        stage: timeline::native::Stage::Lights,
        view: 0,
        ranges: 2,
        slots: vec![],
    };
    let Some(commands) = backend
        .prepare_external(
            surfaces.clone(),
            request,
            [None; 4],
            &plan,
            TargetCopy::Reference,
            Arc::new(Mutex::new(Box::new(run))),
        )
        .unwrap()
    else {
        return false;
    };
    assert_eq!(commands.len(), 1);
    for command in commands {
        assert_eq!(command.ranges, 2, "both groups on the same target copy");
        backend.commit_external(command);
    }
    true
}
