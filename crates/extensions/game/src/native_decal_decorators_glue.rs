//! The decal decorators' host (docs/graphics/retire-d3d-decal-decorators.md):
//! `render_bridge::native_decal_decorators::Host` over the device.
//!
//! - **A body** ([`Host::begin_body`] to [`Host::end_body`]): a draw scope
//!   around the guest's `0x0004ebd0`. Its one `DrawVertices` is admitted
//!   ([`Admission`]): the resolved draw, its quad list or triangle list
//!   indexed ([`indexed`]), recorded at its attachments as a sorted-layer
//!   range (`IndexedPass::for_sorted_layer`). A committed draw is the
//!   template, with the words the body left ([`Words`]).
//! - **A cut call** ([`Host::holds`], [`Host::draw`]): the template's key
//!   is the call's, and every word the body left still holds (the device's
//!   push-buffer put and draw count among them: nothing ran since). The
//!   template's draw with the placement's own vertices ([`replay`]) is
//!   reserved at the boundary and committed.
//! - **A sampled body**: the draw the cut would have made and the words
//!   before the body, compared with the body's draw and the words it left
//!   ([`draw_differences`], [`Words::differences`]).

use std::sync::{Arc, Mutex};

use d3d::{
    DrawAdmission, DrawScope, DrawSnapshot, ExternalPass, ExternalPlan, ResolvedDraw, StreamBytes,
    StreamData,
};
use engine::memory::GuestMemory;
use gfx::vertex::PrimitiveType;
use native_render::world_layers_gpu::{Cache, IndexedPass};
use render_bridge::native_decal_decorators::{self as stage, Body, Call, Key};
use timeline::native::Stage;

use crate::native_lights_glue::DeviceMemory;

/// The guest words outside D3D's that the body writes: the shader draw's
/// cache (`0x00485b48` to its implementation at `+0x24`), the engine input
/// table pointer (`0x0001bd50`), the stream state `0x0051f0f0` (`0x0001c620`
/// writes up to `+0x2d4`) and the stage textures (`0x0001cf50`).
const GUEST_WORDS: [(&str, u32, usize); 4] = [
    (
        "shader_cache",
        engine::render_loop::layer::SETUP_CONTEXT,
        0x28,
    ),
    ("engine_inputs", 0x0048_5a58, 4),
    ("stream_cache", 0x0051_f0f0, 0x2d8),
    ("texture_cache", 0x0051_f3c8, 0x20),
];

/// What the device and the guest hold between draws: the push buffer's put
/// and the device's resolved draws (they move with any D3D work), D3D's
/// state arrays and constant shadow, and [`GUEST_WORDS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Words {
    put: u32,
    draws: u64,
    regions: Vec<(&'static str, Vec<u8>)>,
}

impl Words {
    /// Read now, with the device's layout.
    pub fn read<B: d3d::Backend>(
        device: &d3d::Device<B>,
        mem: &dyn GuestMemory,
    ) -> Result<Self, &'static str> {
        let layout = &device.config().layout;
        Self::read_at(layout, device.residual_tally().draws, mem)
    }

    fn read_at(
        layout: &d3d::GuestLayout,
        draws: u64,
        mem: &dyn GuestMemory,
    ) -> Result<Self, &'static str> {
        use d3d::layout::{RENDER_STATE_SLOTS, STAGE_SLOTS, STAGES};
        let put = mem
            .u32(layout.device + d3d::layout::device_field::PUT)
            .map_err(|_| "words_unread")?;
        let d3d_words = [
            ("render_state", layout.render_state, 4 * RENDER_STATE_SLOTS),
            (
                "texture_state",
                layout.texture_state,
                4 * STAGES * STAGE_SLOTS,
            ),
            ("dirty", layout.dirty_flags, 4),
            ("streams", layout.streams, 16 * 12),
            ("constants", layout.vs_constants, 192 * 16),
        ];
        let regions = d3d_words
            .into_iter()
            .chain(GUEST_WORDS)
            .map(|(name, at, len)| {
                mem.bytes(at, len)
                    .map(|b| (name, b))
                    .map_err(|_| "words_unread")
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            put,
            draws,
            regions,
        })
    }

    /// The regions where `after` differs from `self`; the put and the
    /// draw count are left out (a body moves them).
    pub fn differences(&self, after: &Words) -> Vec<String> {
        self.regions
            .iter()
            .zip(&after.regions)
            .filter(|(a, b)| a != b)
            .map(|((name, _), _)| format!("words_{name}"))
            .collect()
    }
}

/// The body's draw, admitted.
struct Template {
    generation: u64,
    key: Key,
    draw: ResolvedDraw,
    words: Words,
}

/// The fields where `drawn` (a sampled body's draw) differs from
/// `expected` (the draw the cut would have made).
pub fn draw_differences(expected: &ResolvedDraw, drawn: &ResolvedDraw) -> Vec<String> {
    let mut out = Vec::new();
    let mut differ = |name: &str, same: bool| {
        if !same {
            out.push(format!("draw_{name}"));
        }
    };
    differ("entry", expected.entry == drawn.entry);
    differ(
        "primitive",
        expected.primitive == drawn.primitive && expected.indices == drawn.indices,
    );
    differ(
        "vertices",
        expected.vertex_count == drawn.vertex_count
            && expected.layout == drawn.layout
            && expected.streams == drawn.streams,
    );
    differ(
        "state",
        expected.state == drawn.state
            && expected.raw == drawn.raw
            && expected.extras == drawn.extras,
    );
    differ("target", expected.target == drawn.target);
    differ(
        "textures",
        expected.textures == drawn.textures && expected.texture_info == drawn.texture_info,
    );
    differ("shader", expected.shader == drawn.shader);
    differ(
        "viewport",
        expected.viewport == drawn.viewport
            && expected.clip == drawn.clip
            && expected.clip_rects == drawn.clip_rects
            && expected.hw == drawn.hw,
    );
    differ(
        "lens",
        expected.ui_fit == drawn.ui_fit
            && expected.retail_constants == drawn.retail_constants
            && expected.glyph_cache == drawn.glyph_cache,
    );
    out
}

/// The template's draw with the call's `vertices` (its count of the
/// template's stride): what the body would resolve for this placement.
pub fn replay(
    template: &ResolvedDraw,
    call: &Call,
    vertices: Vec<u8>,
) -> Result<ResolvedDraw, &'static str> {
    let key = call.key.as_ref().ok_or("type")?;
    if template.streams.len() != 1 || template.layout.streams.len() != 1 {
        return Err("streams");
    }
    let stride = template.streams[0].stride;
    if stride != u32::from(key.stream.stride) || vertices.len() != (call.count * stride) as usize {
        return Err("stride");
    }
    let mut draw = template.clone();
    draw.streams = vec![StreamBytes {
        data: StreamData::Owned(vertices),
        stride,
    }];
    draw.vertex_count = call.count;
    Ok(draw)
}

/// `draw` (a `DrawVertices` of a quad or triangle list) as an indexed
/// triangle list: each quad `a b c d` as `a b c` and `a c d`, the Xbox's
/// diagonal (`gfx::vertex::PrimitiveType::to_host`).
pub fn indexed(draw: &ResolvedDraw) -> Result<ResolvedDraw, &'static str> {
    if draw.indices.is_some() {
        return Err("indexed");
    }
    let n = draw.vertex_count;
    let indices: Vec<u32> = match draw.primitive {
        PrimitiveType::QuadList => (0..n / 4)
            .flat_map(|q| [4 * q, 4 * q + 1, 4 * q + 2, 4 * q, 4 * q + 2, 4 * q + 3])
            .collect(),
        PrimitiveType::TriangleList => (0..n / 3 * 3).collect(),
        _ => return Err("topology"),
    };
    if indices.is_empty() {
        return Err("empty");
    }
    let mut out = draw.clone();
    out.primitive = PrimitiveType::TriangleList;
    out.indices = Some(indices.into());
    Ok(out)
}

fn view(mem: &dyn GuestMemory) -> u32 {
    mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX)
        .unwrap_or(u32::MAX)
}

/// The body's one draw, as a sorted-layer range at its attachments.
struct Admission {
    view: u32,
    frame: u64,
    cache: Arc<Cache>,
    /// The draw as resolved, or why it was refused (`admission_<why>`).
    seen: Mutex<Vec<Result<ResolvedDraw, &'static str>>>,
}

impl Admission {
    fn pass(&self, snapshot: &DrawSnapshot<'_>) -> Result<IndexedPass, &'static str> {
        let draw = snapshot.draw;
        if draw.entry != "D3DDevice_DrawVertices" {
            return Err("admission_entry");
        }
        if snapshot
            .sampled_stages
            .iter()
            .enumerate()
            .any(|(n, &used)| {
                used && [snapshot.color_header, snapshot.depth_header]
                    .contains(&snapshot.bound_textures[n])
            })
        {
            return Err("admission_feedback");
        }
        let native = indexed(draw).map_err(|why| match why {
            "topology" => "admission_topology",
            "empty" => "admission_empty",
            _ => "admission_indexed",
        })?;
        IndexedPass::for_sorted_layer(&native, Arc::clone(&self.cache), self.frame)
            .ok_or("admission_pass")
    }
}

impl DrawAdmission for Admission {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let pass = self.pass(snapshot);
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        seen.push(
            pass.as_ref()
                .map(|_| snapshot.draw.clone())
                .map_err(|why| *why),
        );
        if seen.len() > 1 {
            return None;
        }
        let plan = ExternalPlan {
            stage: Stage::Transparents,
            view: self.view,
            ranges: 1,
            slots: Vec::new(),
        };
        Some((plan, Box::new(pass.ok()?) as Box<dyn ExternalPass>))
    }
}

/// A body in progress.
struct Pending {
    scope: Option<DrawScope>,
    admission: Arc<Admission>,
    /// A sampled body: the draw the cut would have made and the words
    /// before the body.
    expected: Option<(Result<ResolvedDraw, &'static str>, Words)>,
}

struct Glue<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    cache: crate::native_map::MapIndexedCache,
    template: Mutex<Option<Arc<Template>>>,
    pending: Mutex<Option<Pending>>,
}

impl<B: d3d::Backend + Send + 'static> Glue<B> {
    fn template(&self) -> Option<Arc<Template>> {
        self.template
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set_template(&self, t: Option<Template>) {
        *self.template.lock().unwrap_or_else(|e| e.into_inner()) = t.map(Arc::new);
    }

    /// The current template for `call`, or `template`.
    fn template_for(&self, call: &Call) -> Result<Arc<Template>, &'static str> {
        let t = self.template().ok_or("template")?;
        if Some(t.key) != call.key || t.generation != render_bridge::native_map_work::generation() {
            return Err("template");
        }
        Ok(t)
    }
}

impl<B: d3d::Backend + Send + 'static> stage::Host for Glue<B> {
    fn frame(&self) -> u64 {
        crate::d3d_diag::presents()
    }

    fn begin_body(&self, mem: &mut dyn GuestMemory, call: &Call, sampled: bool) {
        let expected = sampled.then(|| {
            let draw = self
                .template_for(call)
                .and_then(|t| replay(&t.draw, call, call.vertices(&*mem)?));
            let words = self
                .device
                .lock()
                .ok()
                .and_then(|d| Words::read(&*d, &*mem).ok());
            words.map(|w| (draw, w))
        });
        let admission = Arc::new(Admission {
            view: view(&*mem),
            frame: crate::d3d_diag::presents() + 1,
            cache: self.cache.get(),
            seen: Mutex::new(Vec::new()),
        });
        let scope = self.device.lock().ok().and_then(|mut d| {
            d.begin_draw_scope(
                &mut DeviceMemory(mem),
                Arc::clone(&admission) as Arc<dyn DrawAdmission>,
            )
            .ok()
            .flatten()
        });
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
            scope,
            admission,
            expected: expected.flatten(),
        });
    }

    fn end_body(&self, mem: &mut dyn GuestMemory, call: &Call) -> Body {
        let Some(pending) = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        else {
            self.set_template(None);
            return Body {
                native: Err("scope"),
                differs: Vec::new(),
            };
        };
        let Ok(mut d) = self.device.lock() else {
            self.set_template(None);
            return Body {
                native: Err("device"),
                differs: Vec::new(),
            };
        };
        let committed = match pending.scope {
            Some(scope) => d
                .end_draw_scope(&mut DeviceMemory(mem), scope)
                .unwrap_or(false),
            None => false,
        };
        let words = Words::read(&*d, &*mem);
        drop(d);
        let seen = std::mem::take(
            &mut *pending
                .admission
                .seen
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let drawn = match seen.as_slice() {
            [Ok(draw)] => Ok(draw.clone()),
            [Err(why)] => Err(*why),
            [] => Err("unseen"),
            _ => Err("draws"),
        };
        let native = match (&drawn, committed) {
            (Ok(_), true) => Ok(()),
            (Ok(_), false) => Err("reservation"),
            (Err(why), _) => Err(*why),
        };
        let mut differs = Vec::new();
        if let Some((expected, before)) = &pending.expected {
            match (expected, &drawn) {
                (Ok(expected), Ok(drawn)) => differs.extend(draw_differences(expected, drawn)),
                (Err(why), _) => differs.push(format!("expected_{why}")),
                (_, Err(why)) => differs.push(format!("drawn_{why}")),
            }
            match &words {
                Ok(after) => differs.extend(before.differences(after)),
                Err(why) => differs.push((*why).to_owned()),
            }
        }
        let template = match (native, drawn, words, call.key) {
            (Ok(()), Ok(draw), Ok(words), Some(key)) => Some(Template {
                generation: render_bridge::native_map_work::generation(),
                key,
                draw,
                words,
            }),
            _ => None,
        };
        self.set_template(template);
        Body { native, differs }
    }

    fn holds(&self, mem: &dyn GuestMemory, call: &Call) -> Result<(), &'static str> {
        let t = self.template_for(call)?;
        let d = self.device.lock().map_err(|_| "device")?;
        if Words::read(&*d, mem)? != t.words {
            return Err("state");
        }
        Ok(())
    }

    fn draw(&self, mem: &mut dyn GuestMemory, call: &Call) -> Result<(), &'static str> {
        let t = self.template_for(call)?;
        let draw = replay(&t.draw, call, call.vertices(&*mem)?)?;
        let native = indexed(&draw)?;
        let pass = IndexedPass::for_sorted_layer(
            &native,
            self.cache.get(),
            crate::d3d_diag::presents() + 1,
        )
        .ok_or("pass")?;
        let plan = ExternalPlan {
            stage: Stage::Transparents,
            view: view(&*mem),
            ranges: 1,
            slots: Vec::new(),
        };
        let mut d = self.device.lock().map_err(|_| "device")?;
        if !d.can_external_pass() {
            return Err("device");
        }
        match d.prepare_external_pass(&mut DeviceMemory(mem), plan, Box::new(pass)) {
            Ok(Some(reservation)) => {
                reservation.commit();
                Ok(())
            }
            Ok(None) => Err("boundary"),
            Err(_) => Err("device"),
        }
    }
}

/// Installs the host over the device, unless
/// `impl.render.native-decal-decorators=off` (or the native renderer is off).
pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let Ok(switches) = hooks::Switches::from_env() else {
        return;
    };
    if !stage::on(&switches) {
        return;
    }
    stage::set_host(Arc::new(Glue {
        device: Arc::clone(device),
        cache: crate::native_map::MapIndexedCache::default(),
        template: Mutex::new(None),
        pending: Mutex::new(None),
    }));
}

#[cfg(test)]
mod tests;
