//! Pairs the layer hook with the actual indexed D3D call and state.

use std::cell::Cell;
use std::sync::Arc;

use d3d::backend::{
    DrawAdmission, DrawSnapshot, ExternalPass, ExternalPlan, ExternalTarget, ExternalWrites,
    ResolvedDraw,
};
use d3d::shader::VertexProcessing;
use feature::native_world_layers::{Layer, compare_frames, current};
use native_render::camera::GuestRead;
use native_render::world_layers::{
    DRAW_INDEXED_VERTICES, Geometry, Owner, SKY_INDEXED_RETURN, Site,
};
use native_render::world_layers_gpu::{Cache, IndexedPass};

struct Memory<'a>(&'a runtime::Guest);

impl GuestRead for Memory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

#[derive(Clone, Copy)]
struct DrawContext {
    layer: Layer,
    frame: u64,
    compare: bool,
    geometry: Option<Geometry>,
    target: [u32; 8],
    index_count: Option<u32>,
    return_address: u32,
}

thread_local! {
    static DRAW: Cell<Option<DrawContext>> = const { Cell::new(None) };
}

/// Clears the context even when the D3D handler returns a fault or calls
/// out to the guest before its draw. Such a call re-enters this wrapper.
struct Restore(Option<DrawContext>);

impl Drop for Restore {
    fn drop(&mut self) {
        DRAW.set(self.0);
    }
}

pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &d3d::abi::SharedDevice<B>,
) {
    let native = feature::Switches::from_env().is_ok_and(|s| feature::native_world_layers::on(&s));
    let frames = compare_frames();
    if !native && frames.is_empty() {
        return;
    }
    if native && let Ok(mut device) = device.lock() {
        device.add_draw_admission(Arc::new(Admission::default()));
    }
    rt.edit_dispatch(|dispatch| {
        let index = DRAW_INDEXED_VERTICES;
        let Some(previous) = dispatch.override_handler(index) else {
            return;
        };
        let context_only = dispatch.is_context_only(index);
        let device = Arc::clone(device);
        dispatch.register_override(
            index,
            "boot:native world layers indexed",
            move |cpu, ctx, selector| {
                let context = (|| {
                    let frame = crate::d3d_diag::presents() + 1;
                    let compare = frames.contains(&frame);
                    let current = current();
                    if (!native || current.is_none()) && !compare {
                        return None;
                    }
                    let guest = ctx.memory();
                    let mem = Memory(&guest);
                    let return_address = mem.read_u32(cpu.esp)?;
                    let layer = if return_address == SKY_INDEXED_RETURN {
                        Layer {
                            number: 15,
                            view: u32::MAX,
                            caller_return: return_address,
                            site: Site::Sky,
                            list: 0,
                            stencil_off: false,
                            texaccum: None,
                        }
                    } else {
                        current?
                    };
                    let target = if compare {
                        device.lock().ok()?.bound_target()
                    } else {
                        [0; 8]
                    };
                    Some(DrawContext {
                        layer,
                        frame,
                        compare,
                        target,
                        geometry: Geometry::read(&mem, cpu.esp, layer.number),
                        index_count: cpu.esp.checked_add(8).and_then(|a| mem.read_u32(a)),
                        return_address,
                    })
                })();
                let _restore = Restore(DRAW.replace(context));
                Ok(previous.invoke(cpu, ctx, selector))
            },
        );
        if context_only {
            dispatch.mark_context_only(index);
        }
    });
}

#[derive(Default)]
struct Admission {
    cache: Arc<Cache>,
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let c = DRAW.get()?;
        let geometry = c.geometry?;
        let layer = c.layer;
        let draw = snapshot.draw;
        if layer.site != Site::Scene
            || layer.number != 1
            || layer.list == 0
            || draw.entry != "D3DDevice_DrawIndexedVertices"
            || c.index_count? as usize != draw.indices.as_ref()?.len()
        {
            return None;
        }
        let accum = layer.texaccum?;
        if accum.surface? != snapshot.color_header {
            return None;
        }
        let stage = match geometry.owner {
            Owner::Cluster(_) | Owner::Placement(_) => timeline::native::Stage::World,
            Owner::Object { .. } => timeline::native::Stage::Objects,
        };
        let pass = IndexedPass::new(draw, Arc::clone(&self.cache), c.frame, None)?;
        Some((
            ExternalPlan {
                stage,
                view: layer.view,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(Replacement {
                pass,
                context: c,
                geometry,
                color: snapshot.color_header,
                depth: snapshot.depth_header,
            }),
        ))
    }
}

struct Replacement {
    pass: IndexedPass,
    context: DrawContext,
    geometry: Geometry,
    color: u32,
    depth: u32,
}

impl ExternalPass for Replacement {
    fn encode(&mut self, _target: ExternalTarget<'_>) {}

    fn writes(&self) -> ExternalWrites {
        self.pass.writes()
    }

    fn sampled(&self) -> Vec<u32> {
        self.pass.sampled()
    }

    fn prepare_copy(&mut self, target: &ExternalTarget<'_>) -> Result<(), &'static str> {
        self.pass.prepare_copy(target)
    }

    fn record_ranges(&mut self, target: ExternalTarget<'_>) -> usize {
        let (copy, size) = (target.copy, target.size);
        let ranges = self.pass.record_ranges(target);
        if self.context.compare && ranges != 0 {
            eprintln!(
                "native-world-layers: recorded frame={} layer={} view={} geometry={:?} copy={copy:?} size={size:?} guest={:#010x}/{:#010x}",
                self.context.frame,
                self.context.layer.number,
                self.context.layer.view,
                self.geometry,
                self.color,
                self.depth
            );
        }
        ranges
    }
}

pub(crate) fn comparing() -> bool {
    DRAW.get().is_some_and(|c| c.compare)
}

/// A sampled level resolved by D3D to the in-place colour surface, as
/// opposed to a decoded bitmap or a snapshot.
pub(crate) fn sample_surface(surface: impl std::fmt::Debug, texture_id: u64) {
    if let Some(c) = DRAW.get().filter(|c| c.compare) {
        eprintln!(
            "native-world-layers: layer={} indexed_return={:#010x} in_place_sample texture_id={texture_id} surface={surface:?}",
            c.layer.number, c.return_address
        );
    }
}

pub(crate) fn compare_draw(
    frame: u64,
    draw: &ResolvedDraw,
    color: impl std::fmt::Debug,
    depth: impl std::fmt::Debug,
    textures: [Option<u64>; 4],
    outcome: &Result<d3d::backend::DrawOutcome, String>,
) {
    let Some(c) = DRAW.get().filter(|c| c.compare) else {
        return;
    };
    // The entry drains older push writes before it issues the indexed
    // draw. Those earlier draws only belong to their original callers.
    if draw.entry != "D3DDevice_DrawIndexedVertices" {
        return;
    }
    let layer = c.layer;
    let state = &draw.state.render;
    let z_max = gfx::renderer::d3dz_max(draw.target.depth_format);
    let w_clip = d3d::nv2a::w_clip_range(&draw.hw, z_max);
    let w_buffer = d3d::nv2a::w_buffered(draw);
    let shader = match &draw.shader.vertex {
        VertexProcessing::Program { microcode, .. } => Some(microcode.words_hash()),
        VertexProcessing::FixedFunction { .. } => None,
    };
    let texaccum = layer.texaccum.map(|t| {
        let stages = draw.textures.map(|header| header == Some(t.texture));
        (t, t.surface == Some(c.target[0]), stages)
    });
    eprintln!(
        "native-world-layers: d3d frame={frame} layer={} site={:?} layer_return={:#010x} list={:#010x} indexed_return={:#010x} geometry={:?} primitive={:?} indices={:?} vertices={} vs={shader:x?} inputs={:#06x} attachments={color:?}/{depth:?} guest={:#010x}/{:#010x} texaccum={texaccum:?} texture_ids={textures:?} clip={:?} pixels={:?} w={w_buffer} w_clip={w_clip:?} clamp={} z={:?}/{:?}/{} stencil_off={} stencil={:?} color_write={:?} cull={:?}/{:?} alpha={}/{:?}/{} {outcome:?}",
        layer.number,
        layer.site,
        layer.caller_return,
        layer.list,
        c.return_address,
        c.geometry,
        draw.primitive,
        c.index_count,
        draw.vertex_count,
        draw.shader.provided_inputs,
        c.target[0],
        c.target[1],
        draw.clip,
        (
            draw.target.width,
            draw.target.height,
            draw.target.depth_format
        ),
        d3d::nv2a::draw_clamps_depth(draw),
        state.depth_mode,
        state.depth_func,
        state.depth_write,
        layer.stencil_off,
        state.stencil,
        state.color_write,
        state.cull_mode,
        state.front_face,
        state.alpha_test_enable,
        state.alpha_func,
        state.alpha_ref,
    );
}
