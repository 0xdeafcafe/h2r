//! Stage 3's binding adapter. The guest runs its sorted handlers and D3D
//! resolves their indexed draws normally. At that boundary we can reuse
//! one immutable interleaved stream without guessing shader state or
//! texture residency. The backend still owns ordering and attachments.

use std::collections::HashMap;
use std::sync::OnceLock;

use d3d::device::IndexedGeometry;
use d3d::{CachedBytes, ResolvedDraw, StreamBytes, StreamData};
use feature::native_transparents::{self, MapGeneration};
use gfx::repack::{FetchMode, RepackPlan};
use gfx::renderer::VertexStream;
use gfx::vertex::{PrimitiveType, VertexLayout};
use native_render::camera::GuestRead;
use native_render::transparent::{Entry, Source};
use native_render::transparent_streams;

use crate::native_world_glue::parse_probe;

pub(crate) fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !feature::Switches::from_env().is_ok_and(|s| native_transparents::on(&s)) {
        return;
    }
    if let Ok(mut device) = device.lock() {
        let mut cache = Cache::default();
        device.set_scoped_indexed_geometry(
            native_transparents::in_indexed_handler,
            Box::new(move |mem, esp, draw| {
                let (map, entry) = native_transparents::indexed_entry(&ReadMemory(mem), esp)?;
                cache.geometry(map, entry, draw)
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
    compared: u64,
}

impl Cache {
    fn use_map(&mut self, generation: MapGeneration) {
        if self.generation != Some(generation) {
            self.by_streams.clear();
            self.bytes = 0;
            self.generation = Some(generation);
        }
    }

    fn geometry(&mut self, generation: MapGeneration, entry: Entry, draw: &ResolvedDraw) -> Option<IndexedGeometry> {
        self.use_map(generation);
        let d3d::shader::VertexProcessing::Program { microcode, .. } = &draw.shader.vertex else {
            return None;
        };
        if !d3d::nv2a::w_buffered(draw)
            || !draw.state.bad.is_empty()
            || draw.indices.is_none()
            || !matches!(draw.primitive, PrimitiveType::TriangleList | PrimitiveType::TriangleStrip)
        {
            return None;
        }
        let diagnostics = diagnostics();
        if let Some(at) = diagnostics.probe {
            probe(entry, microcode, draw, at);
        }
        if draw.streams.len() < 2 {
            return None;
        }
        let key = StreamKey::new(&draw.streams, draw.vertex_count)?;
        let binding = self.binding(key, &draw.layout, &draw.streams)?;
        let result = IndexedGeometry { layout: binding.layout.clone(), stream: binding.stream.clone() };
        if let Some(compare) = diagnostics.compare.filter(|c| c.matches(entry)) {
            let seen = self.compared;
            self.compared = seen.saturating_add(1);
            if seen == compare.after {
                describe(entry, draw, &result);
            }
            // Keep the original vertex binding for this entry during comparison.
            return None;
        }
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

#[derive(Clone, Copy)]
struct Compare {
    source: Source,
    part: u8,
    record: Option<u16>,
    after: u64,
}

impl Compare {
    fn matches(self, entry: Entry) -> bool {
        entry.source == self.source && entry.part == self.part && self.record.is_none_or(|r| entry.record == r)
    }
}

fn parse_compare(value: &str) -> Option<Compare> {
    let (value, after) = match value.split_once('@') {
        Some((v, after)) => (v, after.trim().parse().ok()?),
        None => (value, 0),
    };
    let mut fields = value.split(':').map(str::trim);
    let kind = fields.next()?;
    let id = fields.next()?;
    let source = match kind {
        "cluster" => Source::Cluster(id.parse().ok()?),
        "placement" => Source::Placement(id.parse().ok()?),
        "object" => Source::Object(match id.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => id.parse().ok()?,
        }),
        _ => return None,
    };
    let part = fields.next()?.parse().ok()?;
    let record = fields.next().map(str::parse).transpose().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(Compare { source, part, record, after })
}

struct Diagnostics {
    compare: Option<Compare>,
    probe: Option<(f32, f32)>,
}

fn diagnostics() -> &'static Diagnostics {
    static CONFIG: OnceLock<Diagnostics> = OnceLock::new();
    CONFIG.get_or_init(|| Diagnostics {
        compare: std::env::var("H2_NATIVE_TRANSPARENTS_COMPARE").ok().as_deref().and_then(parse_compare),
        probe: std::env::var("H2_NATIVE_TRANSPARENTS_PROBE")
            .ok()
            .as_deref()
            .and_then(parse_probe)
            .map(|p| p.at),
    })
}

fn describe(entry: Entry, draw: &ResolvedDraw, native: &IndexedGeometry) {
    let strides: Vec<_> = draw.streams.iter().map(|s| s.stride).collect();
    let original = RepackPlan::with_mode(&draw.layout, &strides, FetchMode::Gpu);
    let packed = RepackPlan::with_mode(&native.layout, &[native.stream.stride], FetchMode::Gpu);
    let clip = d3d::nv2a::w_clip_range(&draw.hw, gfx::renderer::d3dz_max(draw.target.depth_format));
    eprintln!(
        "native-transparents compare {:?} part {} record {} shader {:#010x}, {:?}, {} indices",
        entry.source, entry.part, entry.record, entry.shader, draw.primitive,
        draw.indices.as_deref().map_or(0, |indices| indices.len()),
    );
    eprintln!("  D3D streams {:?}, native streams {:?}", original.streams, packed.streams);
    eprintln!("  shared W clip {clip:?}, target {:?}, scissor {:?}", draw.target, draw.clip);
    eprintln!("  shared c[-96..96] {:?}, inputs {:#06x}", draw.shader.constants, draw.shader.provided_inputs);
    eprintln!("  shared render state {:?}", draw.state.render);
    eprintln!("  shared stage textures {:?}, stage states {:?}", draw.texture_info, draw.state.stages);
}

fn probe(entry: Entry, words: &[u32], draw: &ResolvedDraw, at: (f32, f32)) {
    let Some(indices) = draw.indices.as_deref() else { return };
    let streams: Vec<_> = draw.streams.iter().map(|s| VertexStream { data: &s.data, stride: s.stride }).collect();
    let Some(vertices) = transparent_streams::screen_vertices(
        words, &draw.shader.constants, draw.shader.vertex_defaults, &draw.layout, &streams, draw.vertex_count,
    ) else { return };
    let step = match draw.primitive {
        PrimitiveType::TriangleList => 3,
        PrimitiveType::TriangleStrip => 1,
        _ => return,
    };
    let pixel = (at.0 + 0.5, at.1 + 0.5);
    let hit = indices.windows(3).step_by(step).filter_map(|t| {
        let project = |i| vertices.get(i as usize).copied().flatten();
        let vertices = [project(t[0])?, project(t[1])?, project(t[2])?];
        crate::native_world_glue::covers(&vertices, pixel)
    }).min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((depth, edge)) = hit {
        eprintln!(
            "native-transparents probe {at:?}: {:?} part {} record {} shader {:#010x}, triangle depth {depth:.3}, edge {edge:.2} px",
            entry.source, entry.part, entry.record, entry.shader,
        );
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
