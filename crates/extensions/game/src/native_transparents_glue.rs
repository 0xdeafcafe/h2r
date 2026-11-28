//! Stage 3's binding adapter. The guest runs its sorted handlers and D3D
//! resolves their indexed draws normally. At that boundary we can reuse
//! one immutable interleaved stream without guessing shader state or
//! texture residency. The backend still owns ordering and attachments.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use d3d::device::IndexedGeometry;
use d3d::{CachedBytes, ResolvedDraw, StreamBytes, StreamData};
use render_bridge::native_transparents::{self, MapGeneration};
use gfx::renderer::VertexStream;
use gfx::vertex::{PrimitiveType, VertexLayout};
use native_render::camera::GuestRead;
use native_render::transparent_streams;

use native_render::world_layers_gpu::{Cache as IndexedCache, IndexedPass};
use timeline::native::{Metric, ReservationCause, Stage};

struct Walk<B: d3d::Backend> {
    state: crate::native_world_glue::State,
    device: d3d::abi::SharedDevice<B>,
    cache: Arc<crate::native_map::MapIndexedCache>,
    packed: Arc<Mutex<Cache>>,
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
    fn sorted_pools(&self, mem: &dyn engine::memory::GuestMemory) -> Option<Arc<std::collections::BTreeSet<u32>>> {
        let state = self.state.lock().ok()?;
        let ready = crate::native_world_glue::current_ready(&state, mem)?;
        Some(Arc::clone(&ready.water.get()?.0.pools))
    }

    fn retire_bsp(&self) {
        let packed = self.packed.lock().unwrap_or_else(|e| e.into_inner()).take_map();
        if !packed.by_streams.is_empty() {
            crate::native_map::retire_owned(timeline::setup::Stage::World, packed);
        }
    }

    fn draw(
        &self, mem: &mut dyn engine::memory::GuestMemory, stage: Stage, range: native_transparents::IndexRange,
    ) -> Result<(), ReservationCause> {
        let view = mem.u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
        let primitive = match &range.topology {
            native_transparents::IndexTopology::List => PrimitiveType::TriangleList,
            native_transparents::IndexTopology::Strip => PrimitiveType::TriangleStrip,
        };
        let cache = self.cache.get();
        let candidate = IndexedRange { stage, view, frame: crate::d3d_diag::presents() + 1, cache: &cache };
        // The device names the first step the draw did not pass.
        let mut device = self.device.lock().map_err(|_| ReservationCause::Error)?;
        match device.try_native_indexed_draw_why(
            &mut crate::native_lights_glue::DeviceMemory(mem), primitive, range.count, range.indices, &candidate,
        ) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(why)) => Err(match why {
                d3d::NativeRefusal::Boundary => ReservationCause::Boundary,
                d3d::NativeRefusal::Geometry => ReservationCause::Geometry,
                d3d::NativeRefusal::Admission => ReservationCause::Admission,
                d3d::NativeRefusal::Binding => ReservationCause::Binding,
                d3d::NativeRefusal::Resources => ReservationCause::Resources,
                d3d::NativeRefusal::Sink => ReservationCause::Sink,
                d3d::NativeRefusal::Backend => ReservationCause::Backend,
            }),
            Err(_) => Err(ReservationCause::Error),
        }
    }
}


pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>, state: crate::native_world_glue::State) {
    let switches = hooks::Switches::from_env().unwrap_or_default();
    if !native_transparents::on(&switches) {
        return;
    }
    let packed = Arc::new(Mutex::new(Cache::default()));
    let indexed = Arc::new(crate::native_map::MapIndexedCache::default());
    native_transparents::set_walk_host(Arc::new(Walk {
        state,
        device: device.clone(),
        cache: Arc::clone(&indexed),
        packed: Arc::clone(&packed),
    }));
    if let Ok(mut device) = device.lock() {
        device.set_indexed_admission(Box::new(move |mem, esp, snapshot| {
            if !native_transparents::layered_indexed_call(&ReadMemory(mem), esp) {
                return None;
            }
            let pass = IndexedPass::for_sorted_layer(
                snapshot.draw, indexed.get(), crate::d3d_diag::presents() + 1,
            )?;
            let view = mem.read_u32(engine::render_loop::occlusion::guest::VIEW_INDEX).unwrap_or(u32::MAX);
            Some((d3d::ExternalPlan {
                stage: Stage::Transparents,
                view,
                ranges: 1,
                slots: Vec::new(),
            }, Box::new(pass) as Box<dyn d3d::ExternalPass>))
        }));
        device.set_scoped_indexed_geometry(
            || crate::native_world_layers_glue::pool_issue().is_none() && native_transparents::in_indexed_handler(),
            Box::new(move |mem, esp, draw| {
                // Retained pool admissions compare the original stream layout.
                if crate::native_world_layers_glue::pool_issue().is_some() { return None; }
                let (map, _) = native_transparents::indexed_entry(&ReadMemory(mem), esp)?;
                packed.lock().ok()?.geometry(map, draw)
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

const PACKED_BYTES_BUDGET: usize = 16 * 1024 * 1024;
const PACKED_BINDINGS_BUDGET: usize = 4_096;
static PACKED_BYTES: mem_watch::Counter = mem_watch::Counter::new("native.transparent_packed_bytes");
static PACKED_BINDINGS: mem_watch::Counter = mem_watch::Counter::new("native.transparent_packed_bindings");

#[derive(Default)]
struct Cache {
    generation: Option<MapGeneration>,
    by_streams: HashMap<StreamKey, Vec<Binding>>,
    bytes: usize,
    bindings: usize,
    byte_probe: Option<mem_watch::Gauge>,
    binding_probe: Option<mem_watch::Gauge>,
    accepted: u64,
}

impl Cache {
    fn take_map(&mut self) -> Self {
        let next = Self { accepted: self.accepted, ..Self::default() };
        std::mem::replace(self, next)
    }

    fn use_map(&mut self, generation: MapGeneration) {
        if self.generation != Some(generation) {
            self.take_map();
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
        let result = self
            .binding(key, &draw.layout, &draw.streams)
            .map(|b| IndexedGeometry { layout: b.layout.clone(), stream: b.stream.clone() })?;
        self.accepted += 1;
        if self.accepted % 1800 == 1 {
            eprintln!("native-transparents: {} indexed draws with native bindings", self.accepted);
        }
        Some(result)
    }

    fn binding(&mut self, key: StreamKey, original: &VertexLayout, sources: &[StreamBytes]) -> Option<&IndexedGeometry> {
        let find = |bindings: &Vec<Binding>| bindings.iter().position(|b| b.original == *original);
        if let Some(existing) = self.by_streams.get(&key).and_then(find) {
            Stage::Transparents.count(Metric::CacheHits, 1);
            return Some(&self.by_streams[&key][existing].packed);
        }
        Stage::Transparents.count(Metric::CacheMisses, 1);
        let mut views = [VertexStream { data: &[], stride: 0 }; 16];
        let views = views.get_mut(..sources.len())?;
        for (view, source) in views.iter_mut().zip(sources) {
            *view = VertexStream { data: &source.data, stride: source.stride };
        }
        let packed = transparent_streams::pack(original, views, key.vertices, PACKED_BYTES_BUDGET)?;
        let size = packed.bytes.len();
        if size + self.bytes > PACKED_BYTES_BUDGET || self.bindings == PACKED_BINDINGS_BUDGET {
            self.by_streams = HashMap::new();
            self.bytes = 0;
            self.bindings = 0;
        }
        let packed = IndexedGeometry {
            layout: packed.layout,
            stream: StreamBytes {
                stride: packed.stride,
                data: StreamData::Cached(CachedBytes::derived(packed.bytes)),
            },
        };
        self.bytes += size;
        self.bindings += 1;
        self.byte_probe.get_or_insert_with(|| PACKED_BYTES.track(0)).set(self.bytes as u64);
        self.binding_probe.get_or_insert_with(|| PACKED_BINDINGS.track(0)).set(self.bindings as u64);
        let bindings = self.by_streams.entry(key).or_default();
        bindings.push(Binding { original: original.clone(), packed });
        bindings.last().map(|b| &b.packed)
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
    fn packed_geometry_releases_its_budget_with_the_retired_map() {
        let mut cache = Cache::default();
        let stream = || StreamBytes {
            stride: 4,
            data: StreamData::Cached(CachedBytes::derived(1.0f32.to_le_bytes().to_vec())),
        };
        let streams = [stream(), stream()];
        let key = StreamKey::new(&streams, 1).unwrap();
        cache.binding(key, &layout(), &streams).unwrap();

        let retired = cache.take_map();
        assert!(cache.by_streams.is_empty());
        assert_eq!(cache.bytes, 0);
        assert_eq!(retired.bytes, 8);
        assert_eq!(retired.byte_probe.as_ref().unwrap().get(), 8);
    }

    #[test]
    fn small_packed_bindings_cannot_grow_to_the_byte_budget_in_cache_entries() {
        let mut cache = Cache::default();
        let layout = layout();
        let stream = || StreamBytes {
            stride: 4,
            data: StreamData::Cached(CachedBytes::derived(1.0f32.to_le_bytes().to_vec())),
        };
        for _ in 0..=PACKED_BINDINGS_BUDGET {
            let sources = [stream(), stream()];
            let key = StreamKey::new(&sources, 1).unwrap();
            cache.binding(key, &layout, &sources).unwrap();
        }
        assert_eq!(cache.by_streams.len(), 1);
        assert_eq!(cache.bindings, 1);
        assert_eq!(cache.binding_probe.as_ref().unwrap().get(), 1);
        assert_eq!(cache.byte_probe.as_ref().unwrap().get(), 8);
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
