//! Binds a resolved draw to its sorted geometry entry and one shadow call.
use crate::native_shadows_draw::{Output, Range, SharedGpu};
use feature::native_shadows::{Entry, Layer};
use native_render::{
    shadow_gpu::MapKind,
    shadows::{self, Geometry, Site},
};
use std::sync::{Arc, Mutex};

pub(super) struct Call {
    pub seq: u64,
    pub frame: u64,
    pub bound: [u32; 8],
    pub identity: [u32; 2],
    pub layer: Layer,
    pub kind: MapKind,
    pub header: u32,
    pub recorded: Option<Arc<std::sync::atomic::AtomicUsize>>,
    pub output: Output,
}
impl Call {
    pub fn supplies(&self, receiver: &Call) -> bool {
        self.layer.layer == 10
            && receiver.layer.layer == 11
            && self.frame == receiver.frame
            && self.identity == receiver.identity
            && self.layer.site == receiver.layer.site
            && self.layer.flags == receiver.layer.flags
            && self.layer.view == receiver.layer.view
            && self.layer.projection == receiver.layer.projection
    }
}
#[derive(Default)]
pub(super) struct State {
    pub active: Option<Arc<Call>>,
    pub previous: Option<Arc<Call>>,
    pub entry: Option<Entry>,
    pub owner: Option<Site>,
    pub identity: Option<[u32; 2]>,
}
pub(super) struct Gate {
    pub state: Mutex<State>,
    pub gpu: Option<SharedGpu>,
    pub compare: bool,
}
impl d3d::DrawAdmission for Gate {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let state = self.state.lock().ok()?;
        let call = state.active.as_ref()?;
        let e = state.entry?;
        let l = &call.layer;
        let draw = snapshot.draw;
        if call.bound[..2] != [snapshot.color_header, snapshot.depth_header] {
            return None;
        }
        let exact_projection = draw.shader.constants[40..49]
            .iter()
            .flatten()
            .map(|v| v.to_bits())
            .eq(l.projection);
        if self.compare {
            let rs = &draw.state.render;
            eprintln!(
                "native-shadows frame={} seq={} layer={} item={} impl={} projection_match={} target={:?} clip={:?}/{:?} depth_clip={:?} clamp={} viewport={:?} bias={:?} stencil={:?} writes={:?} alpha={:?} blend={:?} stages={:?}",
                call.frame,
                call.seq,
                l.layer,
                e.index,
                e.implementation,
                exact_projection,
                draw.target,
                draw.clip,
                draw.clip_rects,
                [draw.hw.clip_min, draw.hw.clip_max],
                rs.depth_clamp,
                draw.viewport,
                (
                    rs.solid_offset_enable,
                    rs.depth_bias,
                    rs.depth_bias_slope_scale
                ),
                rs.stencil,
                rs.color_write,
                (rs.alpha_test_enable, rs.alpha_func, rs.alpha_ref),
                (rs.blend_enable, rs.src_blend, rs.dst_blend),
                draw.texture_info
            );
            return None;
        }
        let gpu = self.gpu.as_ref()?;
        if (!exact_projection && l.layer != 14)
            || draw.clip.is_none()
            || draw.hw.zpass_count_enable
            || !draw.state.bad.is_empty()
            || !matches!(
                draw.shader.vertex,
                d3d::shader::VertexProcessing::Program { .. }
            )
            || draw.shader.pixel_shader.is_none()
            || draw.target.width == 0
            || draw.target.height == 0
        {
            return None;
        }
        let item = l.items.iter().find(|i| {
            i.index == u32::from(e.index) && i.argument == e.argument && i.shader == e.shader
        })?;
        let object = match item.geometry {
            Geometry::Other { .. } => return None,
            Geometry::Model { .. } => true,
            Geometry::Bsp(_) => false,
        };
        let map = match l.layer {
            10 if shadows::generation(l.flags, object, item.marked)
                == Some(usize::from(e.implementation)) =>
            {
                None
            }
            11 => {
                let choice =
                    shadows::application(l.flags, e.definition_bit5?, e.int4?, item.marked)?;
                if choice != usize::from(e.implementation) {
                    return None;
                }
                let source = state.previous.as_ref()?;
                if !source.supplies(call) {
                    return None;
                }
                let samples_map = draw
                    .texture_info
                    .iter()
                    .flatten()
                    .any(|i| i.header == source.header);
                if !samples_map && choice != 2 {
                    return None;
                }
                Some((Arc::clone(&source.output), source.kind, source.header))
            }
            // 0x4cbf0 keeps 0x15d70's template choice for this layer.
            // The sorted entry and resolved packet carry that exact choice.
            14 if l.site == Site::Light
                && l.target == 1
                && l.flags & 4 != 0
                && e.implementation == 0 =>
            {
                None
            }
            _ => return None,
        };
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Shadows,
            view: l.view,
            ranges: draw.clip_rects.len().max(1),
            slots: Vec::new(),
        };
        Some((
            plan,
            Box::new(Range {
                gpu: Arc::clone(gpu),
                draw: Arc::new(draw.clone()),
                packets: Vec::new(),
                recorded: call.recorded.clone(),
                map,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(layer: u32) -> Call {
        Call {
            seq: u64::from(layer),
            frame: 3,
            bound: [0; 8],
            identity: [1, 2],
            layer: Layer {
                site: Site::Light,
                layer,
                target: 9,
                view: 1,
                flags: 4,
                items: vec![],
                projection: [0; 36],
            },
            kind: MapKind::Depth,
            header: 9,
            output: Default::default(),
            recorded: None,
        }
    }

    #[test]
    fn receiver_requires_its_own_light_view_frame_projection_and_site() {
        let source = call(10);
        assert!(source.supplies(&call(11)));
        for field in 0..7 {
            let mut receiver = call(11);
            match field {
                0 => receiver.identity[0] += 1,
                1 => receiver.layer.flags = 0,
                2 => receiver.layer.view += 1,
                3 => receiver.frame += 1,
                4 => receiver.layer.projection[35] ^= 1,
                5 => receiver.layer.site = Site::AdditionalCaster,
                _ => receiver.layer.layer = 14,
            }
            assert!(!source.supplies(&receiver));
        }
    }
}
