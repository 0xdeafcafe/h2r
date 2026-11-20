//! Stage 3's binding adapter. The guest runs its sorted handlers and D3D
//! resolves their indexed draws normally. At that boundary we can reuse
//! one immutable interleaved stream without guessing shader state or
//! texture residency. The backend still owns ordering and attachments.

use std::collections::HashMap;
use std::sync::Arc;

use d3d::device::IndexedGeometry;
use d3d::{CachedBytes, ResolvedDraw, StreamBytes, StreamData};
use render_bridge::native_transparents::{self, MapGeneration};
use gfx::renderer::VertexStream;
use gfx::vertex::{PrimitiveType, VertexLayout};
use native_render::camera::GuestRead;
use native_render::transparent_streams;

use native_render::world_layers_gpu::{Cache as IndexedCache, IndexedPass};
use timeline::native::Stage;

struct Walk<B: d3d::Backend> {
    device: d3d::abi::SharedDevice<B>,
    cache: Arc<IndexedCache>,
}

struct IndexedRange<'a> {
    stage: Stage,
    frame: u64,
    view: u32,
    cache: &'a Arc<IndexedCache>,
}

impl d3d::DrawAdmission for IndexedRange<'_> {
    fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let pass = IndexedPass::new(snapshot.draw, Arc::clone(self.cache), self.frame, None)?;
        Some((d3d::ExternalPlan {
            stage: self.stage,
            view: self.view,
            ranges: 1,
            slots: Vec::new(),
        }, Box::new(pass)))
    }
}

impl<B: d3d::Backend + Send + 'static> native_transparents::WalkHost for Walk<B> {
    fn draw(&self, mem: &mut dyn engine::memory::GuestMemory, stage: Stage, range: native_transparents::IndexRange) -> bool {
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let candidate = IndexedRange { stage, view, frame: crate::d3d_diag::presents() + 1, cache: &self.cache };
        let primitive = if range.strip { PrimitiveType::TriangleStrip } else { PrimitiveType::TriangleList };
        self.device.lock().is_ok_and(|mut device| {
            device.try_native_indexed_draw(
                &mut crate::native_lights_glue::DeviceMemory(mem), primitive, range.count, range.indices, &candidate,
            ).ok() == Some(true)
        })
    }
}


pub(crate) fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !native_transparents::on(&switches) {
        return;
    }
    native_transparents::set_walk_host(Arc::new(Walk { device: device.clone(), cache: Arc::default() }));
    if let Ok(mut device) = device.lock() {
        let mut cache = Cache::default();
        device.set_scoped_indexed_geometry(
            native_transparents::in_indexed_handler,
            Box::new(move |mem, esp, draw| {
                let (map, _) = native_transparents::indexed_entry(&ReadMemory(mem), esp)?;
                cache.geometry(map, draw)
            }),
        );
    }
}

struct ReadMemory<'a>(&'a dyn d3d::GuestMemory);

impl GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.read_u32(addr).ok()
    }
}

/// Snapshot ids change when the guest writes a stream or restores the
/// device. Strides and the whole declaration are also part of identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct StreamKey {
    streams: [Option<(u64, u32)>; 16],
    vertices: u32,
}

impl StreamKey {
    fn new(streams: &[StreamBytes], vertices: u32) -> Option<Self> {
        let mut key = Self { streams: [None; 16], vertices };
        key.streams.get(..streams.len())?;
        for (slot, stream) in key.streams.iter_mut().zip(streams) {
            *slot = Some((stream.data.cache_id()?, stream.stride));
        }
        Some(key)
    }
}

struct Binding {
    original: VertexLayout,
    packed: IndexedGeometry,
}

#[derive(Default)]
struct Cache {
    generation: Option<MapGeneration>,
    by_streams: HashMap<StreamKey, Vec<Binding>>,
    bytes: usize,
    accepted: u64,
}

impl Cache {
    fn use_map(&mut self, generation: MapGeneration) {
        if self.generation != Some(generation) {
            self.by_streams.clear();
            self.bytes = 0;
            self.generation = Some(generation);
        }
    }

    fn geometry(&mut self, generation: MapGeneration, draw: &ResolvedDraw) -> Option<IndexedGeometry> {
        self.use_map(generation);
        if !matches!(draw.shader.vertex, d3d::shader::VertexProcessing::Program { .. }) {
            return None;
        }
        if !d3d::nv2a::w_buffered(draw)
            || !draw.state.bad.is_empty()
            || draw.indices.is_none()
            || !matches!(draw.primitive, PrimitiveType::TriangleList | PrimitiveType::TriangleStrip)
        {
            return None;
        }
        if draw.streams.len() < 2 {
            return None;
        }
        let key = StreamKey::new(&draw.streams, draw.vertex_count)?;
        let binding = self.binding(key, &draw.layout, &draw.streams)?;
        let result = IndexedGeometry { layout: binding.layout.clone(), stream: binding.stream.clone() };
        self.accepted += 1;
        if self.accepted % 1800 == 1 {
            eprintln!("native-transparents: {} indexed draws with native bindings", self.accepted);
        }
        Some(result)
    }

    fn binding(&mut self, key: StreamKey, original: &VertexLayout, sources: &[StreamBytes]) -> Option<&IndexedGeometry> {
        const BUDGET: usize = 16 * 1024 * 1024;
        let find = |bindings: &Vec<Binding>| bindings.iter().position(|b| b.original == *original);
        if let Some(existing) = self.by_streams.get(&key).and_then(find) {
            return Some(&self.by_streams[&key][existing].packed);
        }
        let sources: Vec<_> = sources.iter().map(|s| VertexStream { data: &s.data, stride: s.stride }).collect();
        let packed = transparent_streams::pack(original, &sources, key.vertices)?;
        if packed.bytes.len() > BUDGET {
            return None;
        }
        let size = packed.bytes.len();
        if size + self.bytes > BUDGET {
            self.by_streams.clear();
            self.bytes = 0;
        }
        let bindings = self.by_streams.entry(key).or_default();
        bindings.push(Binding {
            original: original.clone(),
            packed: IndexedGeometry {
                layout: packed.layout,
                stream: StreamBytes {
                    stride: packed.stride,
                    data: StreamData::Cached(CachedBytes::derived(packed.bytes)),
                },
            },
        });
        self.bytes += size;
        Some(&bindings.last().expect("just added").packed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::vertex::{StreamLayout, VertexAttribute, VertexDataType, VertexRegister};

    fn layout() -> VertexLayout {
        let stream = |slot, register| StreamLayout {
            stream: slot,
            stride: 4,
            attributes: vec![VertexAttribute {
                register: VertexRegister(register),
                data_type: VertexDataType::Float1,
                offset: 0,
            }],
        };
        VertexLayout { streams: vec![stream(0, 0), stream(2, 5)], pretransformed: false }
    }

    #[test]
    fn a_packed_binding_is_reused_only_for_the_same_snapshot_and_declaration() {
        let mut cache = Cache::default();
        let mut layout = layout();
        let stream = |value: f32| StreamBytes {
            stride: 4,
            data: StreamData::Cached(CachedBytes::derived(value.to_le_bytes().to_vec())),
        };
        let mut streams = [stream(1.0), stream(2.0)];
        let key = StreamKey::new(&streams, 1).unwrap();
        let id = |b: &IndexedGeometry| b.stream.data.cache_id().unwrap();
        let first = id(cache.binding(key, &layout, &streams).unwrap());
        assert_eq!(id(cache.binding(key, &layout, &streams).unwrap()), first);

        layout.streams[1].attributes[0].register = VertexRegister(6);
        let new_layout = id(cache.binding(key, &layout, &streams).unwrap());
        assert_ne!(new_layout, first);

        streams[1] = stream(3.0);
        let key = StreamKey::new(&streams, 1).unwrap();
        let packed = cache.binding(key, &layout, &streams).unwrap();
        assert_ne!(id(packed), new_layout);
        assert_eq!(&packed.stream.data[4..8], &3.0f32.to_le_bytes());
    }
}
