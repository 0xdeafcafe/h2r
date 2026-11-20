//! Vertex inputs prepared for an opaque BSP section. Vertex snapshots are
//! owned by the device's size-limited range cache. Retaining the projection
//! holds neither Xbox references nor a second copy of the section's vertices.

use std::mem::size_of;

use super::*;

const DRAW: &str = "D3DDevice_DrawIndexedVertices";

struct Projection {
    vaf: [u32; 64],
    layout: VertexLayout,
    sources: Vec<StreamSource>,
    // Physical Data words read before state application. Refcounts and
    // fences can change in SetVertexShaderInput without changing Data.
    headers: Vec<Option<u32>>,
    base: u32,
    stale: AtomicBool,
    ranges: Vec<PreparedRange>,
}

struct PreparedRange {
    indices: Option<(u16, u16)>,
    windows: Vec<Option<VertexWindow>>,
}

struct VertexWindow {
    va: u32,
    len: u32,
}

impl Projection {
    fn metadata_bytes(&self) -> usize {
        size_of::<Self>() + 64
            + self.sources.capacity() * size_of::<StreamSource>()
            + self.headers.capacity() * size_of::<Option<u32>>()
            + self.layout.streams.capacity() * size_of::<StreamLayout>()
            + self.ranges.capacity() * size_of::<PreparedRange>()
            + self.layout.streams.iter().map(|s| s.attributes.capacity() * size_of::<VertexAttribute>()).sum::<usize>()
            + self.ranges.iter().map(|r| r.windows.capacity() * size_of::<Option<VertexWindow>>()).sum::<usize>()
    }
}

/// A prepared declaration and the bindings it expects the handler to set.
/// Can be retained across draws. The ordered emit rechecks the actual
/// declaration, bindings, resource headers, index bounds and base vertex.
#[derive(Clone, Default)]
pub struct PreparedWorldVertices(Option<Arc<Projection>>);

/// A section, its vertex implementation, and its cluster/placement. A hit
/// is a candidate until the actual device bindings pass the emit guards.
pub type WorldVertexKey = [u32; 3];

/// Two bounded generations retain recently used projections across frames.
/// A projection owns addresses and declaration metadata only; payload is
/// charged to the existing vertex cache.
#[derive(Default)]
pub(super) struct WorldVertexPlans {
    active: HashMap<WorldVertexKey, Arc<Projection>>,
    previous: HashMap<WorldVertexKey, Arc<Projection>>,
    active_bytes: usize,
}

impl WorldVertexPlans {
    fn get(&mut self, key: WorldVertexKey, ranges: &[PreparedIndexRange]) -> Option<PreparedWorldVertices> {
        let p = self.active.get(&key).or_else(|| self.previous.get(&key))?;
        if p.stale.load(AtomicOrdering::Relaxed) || p.ranges.len() != ranges.len()
            || !p.ranges.iter().zip(ranges).all(|(r, live)| r.indices == live.window()) {
            if let Some(old) = self.active.remove(&key) {
                self.active_bytes -= old.metadata_bytes();
            }
            self.previous.remove(&key);
            return None;
        }
        let out = PreparedWorldVertices(Some(Arc::clone(p)));
        if let Some(p) = self.previous.remove(&key) { self.insert(key, p); }
        Some(out)
    }

    fn insert(&mut self, key: WorldVertexKey, p: Arc<Projection>) {
        const GENERATION_BYTES: usize = 4 << 20;
        let bytes = p.metadata_bytes();
        if bytes > GENERATION_BYTES { return; }
        if self.active_bytes + bytes > GENERATION_BYTES {
            self.previous = std::mem::take(&mut self.active);
            self.active_bytes = 0;
        }
        self.active_bytes += bytes;
        self.active.insert(key, p);
    }
}

fn window(base: u32, lo: u16, hi: u16, data: u32, source: StreamSource)
    -> Result<(u32, usize), D3dError>
{
    base.checked_add(u32::from(hi)).ok_or_else(|| D3dError::invalid(DRAW, "base vertex overflow"))?;
    let count = u32::from(hi) - u32::from(lo) + 1;
    let len = span(DRAW, count, source.stride)?;
    let first = base + u32::from(lo);
    let offset = u64::from(source.offset) + u64::from(first) * u64::from(source.stride);
    let va = at(data, offset)?;
    at(va, len as u64)?;
    Ok((va, len))
}

impl<B: Backend> Device<B> {
    /// Reuse section preparation without reading each descriptor or rescanning
    /// its vertex declaration on every visit. A runtime rebind invalidates
    /// this hint at the emitter, where the actual input is still available.
    pub fn prepare_section_vertices(
        &mut self, mem: &dyn GuestMemory, key: WorldVertexKey, ranges: &[PreparedIndexRange],
        projection: impl FnOnce() -> Option<([u32; 64], VertexLayout, Vec<StreamSource>)>,
    ) -> Result<PreparedWorldVertices, D3dError> {
        if let Some(p) = self.world_vertex_plans.get(key, ranges) { return Ok(p); }
        let out = self.prepare_world_vertices(mem, projection(), ranges)?;
        if let Some(p) = &out.0 && !p.ranges.is_empty() {
            self.world_vertex_plans.insert(key, Arc::clone(p));
        }
        Ok(out)
    }

    /// Prime the same snapshots used by ordinary indexed draws. When a
    /// prospective binding is unavailable, leave the real emitter to read
    /// its eventual input. Streaming or an auxiliary pass may change it.
    pub fn prepare_world_vertices(
        &mut self, mem: &dyn GuestMemory,
        projected: Option<([u32; 64], VertexLayout, Vec<StreamSource>)>,
        ranges: &[PreparedIndexRange],
    ) -> Result<PreparedWorldVertices, D3dError> {
        let Some((vaf, layout, sources)) = projected else { return Ok(Default::default()) };
        let mut p = Projection {
            vaf, layout, sources, headers: Vec::new(), base: 0, stale: AtomicBool::new(false), ranges: Vec::new(),
        };
        if self.vertex_cache.is_some() && self.cfg.upload == UploadPolicy::Tracked {
            p.base = mem.read_u32(self.cfg.layout.device + field::BASE_VERTEX)?;
            p.headers = p.sources.iter().map(|s| {
                ResourceHeader::read(mem, s.buffer).ok()
                    .filter(|h| s.buffer != 0 && h.kind() == ResourceType::VertexBuffer).map(|h| h.data)
            }).collect();
            for range in ranges {
                let indices = range.window();
                let mut windows = Vec::new();
                if let Some((lo, hi)) = indices {
                    windows = p.sources.iter().zip(&p.headers).map(|(&s, &header)| {
                        let data = self.cfg.address_map.to_virtual(header?);
                        let (va, len) = window(p.base, lo, hi, data, s).ok()?;
                        self.prepare_vertex_window(mem, va, len)
                    }).collect();
                }
                p.ranges.push(PreparedRange { indices, windows });
            }
        }
        Ok(PreparedWorldVertices(Some(Arc::new(p))))
    }

    fn prepare_vertex_window(&mut self, mem: &dyn GuestMemory, va: u32, len: usize) -> Option<VertexWindow> {
        let size = len as u32;
        let cache = self.vertex_cache.as_mut()?;
        if cache.needs_static_window(va, size) {
            let bytes = mem.read_vec(va, len).ok()?;
            self.tracker.watch(va, size);
            self.stats.vertex_cache_fills += 1;
            self.stats.vertex_bytes_copied += len as u64;
            count_vertex_bytes(timeline::phase::Counter::VertexCopied, len);
            cache.fill(va, size, self.tracker.generation(), |id| CachedBytes { id, bytes: bytes.into() });
        }
        // Keep the location only. An evicted snapshot releases its bytes
        // immediately, irrespective of how many material plans still exist.
        Some(VertexWindow { va, len: size })
    }

    fn projected_layout_is_bound(&self, mem: &dyn GuestMemory, projection: &Projection)
        -> Result<bool, D3dError>
    {
        if self.vertex_shader & 1 == 0 || projection.layout.streams.len() != projection.sources.len() {
            return Ok(false);
        }
        let active = self.stream_inputs.as_deref().unwrap_or(&self.streams);
        if !projection.layout.streams.iter().zip(&projection.sources)
            .all(|(s, source)| active.get(s.stream as usize) == Some(source)) {
            return Ok(false);
        }
        let at = at(self.vertex_shader & !1, 0x14)?;
        Ok(crate::guest::read_array::<64>(mem, at)? == projection.vaf)
    }

    fn world_program(&mut self, code: Option<&Microcode>) -> VertexProcessing {
        let start = self.program_start;
        let at = (start as usize).saturating_mul(4);
        // The callable format accepts arbitrary shader words. Match the
        // same instruction extent (through the first FINAL) as microcode().
        let microcode = match code.filter(|c| {
            c.ends_at_first_final()
                && at.checked_add(c.len()).and_then(|end| self.program.get(at..end)) == Some(&c[..])
        }) {
            Some(c) => c.clone(),
            None => self.microcode(),
        };
        VertexProcessing::Program { start, microcode }
    }

    /// Resolve one indexed emission after draining older push commands.
    /// Validation is per emission: an earlier subdraw can change memory
    /// underlying a later vertex buffer or install new device state.
    pub fn world_vertex_part(
        &mut self, mem: &dyn GuestMemory, vertices: &PreparedWorldVertices,
        ordinal: usize, range: &PreparedIndexRange, code: Option<&Microcode>,
    ) -> Result<(VertexLayout, VertexProcessing, Vec<StreamBytes>), D3dError> {
        let (lo, hi) = range.window().ok_or_else(|| D3dError::invalid(DRAW, "empty vertex window"))?;
        let base = mem.read_u32(self.cfg.layout.device + field::BASE_VERTEX)?;
        base.checked_add(u32::from(hi)).ok_or_else(|| D3dError::invalid(DRAW, "base vertex overflow"))?;
        let count = u32::from(hi) - u32::from(lo) + 1;
        let first = base + u32::from(lo);
        let mut streams = std::mem::take(&mut self.stream_scratch);
        let result = (|| {
            if let Some(p) = &vertices.0 && self.projected_layout_is_bound(mem, p)? {
                let ready = p.ranges.get(ordinal).filter(|r| {
                    r.indices == Some((lo, hi)) && p.base == base &&
                        self.cfg.upload == UploadPolicy::Tracked &&
                        self.vertex_cache.is_some()
                });
                for (i, source) in p.sources.iter().enumerate() {
                    if let Some(w) = ready.and_then(|r| r.windows.get(i)?.as_ref())
                    {
                        let header = ResourceHeader::read(mem, source.buffer)?;
                        if header.kind() == ResourceType::VertexBuffer && p.headers[i] == Some(header.data) {
                            streams.push(self.cached_stream(mem, source.buffer, w.va, w.len as usize, source.stride)?);
                            continue;
                        }
                    }
                    p.stale.store(true, AtomicOrdering::Relaxed);
                    streams.push(self.read_stream(mem, DRAW, source, first, count)?);
                }
                Ok((p.layout.clone(), self.world_program(code)))
            } else {
                if let Some(p) = &vertices.0 { p.stale.store(true, AtomicOrdering::Relaxed); }
                let (layout, vertex, sources) = self.vertex_setup(mem, DRAW)?;
                for source in &sources {
                    streams.push(self.read_stream(mem, DRAW, source, first, count)?);
                }
                Ok((layout, vertex))
            }
        })();
        match result {
            Ok((layout, vertex)) => Ok((layout, vertex, streams)),
            Err(e) => { self.recycle_streams(streams); Err(e) }
        }
    }
}
