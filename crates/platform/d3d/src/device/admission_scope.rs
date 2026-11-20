//! A producer boundary drain that leaves interrupt callbacks queued.
use super::*;
impl<B: Backend> Device<B> {
    /// Drain only already-published commands. False means an interrupt
    /// callback must run first; it stays queued for the ordinary D3D entry.
    /// A caller must cancel its admission scope on false or error.
    pub fn drain_admission_boundary(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<bool, D3dError> {
        if let Some(cb) = self.drain_until_callback(mem)? {
            self.undelivered = Some(cb);
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

impl<B: Backend> Device<B> {
    fn native_producer_ready(&mut self, mem: &mut dyn GuestMemory) -> Result<bool, D3dError> {
        Ok(self.native_boundary_ready(mem)? && !self.immediate.is_open())
    }

    fn native_boundary_ready(&mut self, mem: &mut dyn GuestMemory) -> Result<bool, D3dError> {
        if !self.created || !self.can_external_pass() || self.swallowing.is_some()
            || self.scoped_admission.is_some()
        {
            return Ok(false);
        }
        Ok(self.drain_admission_boundary(mem)? && !self.hw.zpass_count_enable)
    }

    /// Submit only the native recording. Refusal leaves the caller's retail
    /// emission in charge of issuing the draw.
    fn try_native_producer(
        &mut self,
        mem: &mut dyn GuestMemory,
        mut parts: DrawParts,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        let mut streams = std::mem::take(&mut parts.streams);
        let mut committed = false;
        let result = self.submit_inner(mem, parts, &mut streams, Some((admission, &mut committed)), None);
        self.recycle_streams(streams);
        result.map(|()| committed)
    }

    /// Record a known array draw using the same stream source and state as
    /// the guest API. Texture resolution and target identity are shared
    /// with D3D. A failed native reservation issues no draw.
    pub fn try_native_array_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        start: u32,
        count: u32,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        const ENTRY: &str = "D3DDevice_DrawVertices";
        if !self.native_producer_ready(mem)? || count == 0 {
            return Ok(false);
        }
        vertex_limit(ENTRY, count)?;
        let (layout, vertex, sources) = self.vertex_setup(mem, ENTRY)?;
        let mut streams = Vec::with_capacity(sources.len());
        for s in &sources {
            streams.push(self.read_stream(mem, ENTRY, s, start, count)?);
        }
        let words = Arc::new(self.state(mem)?);
        let parts = DrawParts {
            entry: ENTRY,
            indexed_esp: None,
            primitive,
            vertex_count: count,
            layout,
            vertex,
            streams,
            indices: None,
            words: words.clone(),
        };
        let committed = self.try_native_producer(mem, parts, admission)?;
        if committed {
            self.pushed = Some(words);
        }
        Ok(committed)
    }

    /// A geometry producer already selected this exact index range.
    /// Resolve the same backing bytes as the guest draw; commit only if
    /// the supplied native owner can record the whole range.
    pub fn try_native_indexed_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        count: u32,
        indices: u32,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        if !self.native_producer_ready(mem)? {
            return Ok(false);
        }
        let Some(parts) = self.indexed_parts(mem, primitive, count, indices, None, false)? else {
            return Ok(false);
        };
        let words = parts.words.clone();
        let committed = self.try_native_producer(mem, parts, admission)?;
        if committed {
            self.pushed = Some(words);
        }
        Ok(committed)
    }

    /// Record inline array bytes the producer already knows. Apply attribute
    /// carry tentatively so the shader resolves the same unwritten values.
    /// A refused recording restores the live registers before guest fallback.
    pub fn try_native_inline_draw(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        data: Vec<u8>,
        carried: &[Option<[f32; 4]>; 16],
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        const ENTRY: &str = "inline Begin/End";
        if !self.native_producer_ready(mem)? || self.vertex_shader & 1 == 0 {
            return Ok(false);
        }
        let (attributes, size) = inline_array_layout(&self.hw.vertex_format)
            .map_err(|e| D3dError::unimplemented(ENTRY, e))?;
        if size == 0 || data.is_empty() || !data.len().is_multiple_of(size as usize) {
            return Ok(false);
        }
        let vertex_count = u32::try_from(data.len() / size as usize)
            .map_err(|_| D3dError::invalid(ENTRY, "too many inline vertices"))?;
        vertex_limit(ENTRY, vertex_count)?;
        let last = &data[data.len() - size as usize..];
        let vertex = VertexProcessing::Program {
            start: self.program_start,
            microcode: self.microcode(),
        };
        let words = self.pushed_state(mem)?;
        let initial = *self.immediate.registers();
        for (n, value) in carried.iter().enumerate() {
            if let Some(value) = value {
                self.immediate.carry(n, *value);
            }
        }
        for a in &attributes {
            let o = a.offset as usize;
            let value = attribute_value(a.data_type, &last[o..o + a.data_type.byte_size() as usize]);
            self.immediate.carry(a.register.0 as usize, value);
        }
        let parts = DrawParts {
            entry: ENTRY,
            indexed_esp: None,
            primitive,
            vertex_count,
            layout: VertexLayout {
                streams: vec![StreamLayout { stream: 0, attributes, stride: size }],
                pretransformed: false,
            },
            vertex,
            streams: vec![StreamBytes { data: data.into(), stride: size }],
            indices: None,
            words,
        };
        let result = self.try_native_producer(mem, parts, admission);
        if !matches!(result, Ok(true)) {
            self.immediate = Immediate::with_registers(initial);
        }
        result
    }

    /// Record immediate geometry before an API Begin and its vertex setters.
    /// Each vertex supplies its writes in order, with position last.
    pub fn try_native_immediate_draw<const N: usize>(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        vertices: &[[(u8, [f32; 4]); N]],
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        self.try_native_register_draw(mem, primitive, vertices, admission, false)
    }

    /// As above, for a producer that would write Begin, vertices and End
    /// directly into the push buffer. It observes pushed render state.
    pub fn try_native_pushed_draw<const N: usize>(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        vertices: &[[(u8, [f32; 4]); N]],
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        self.try_native_register_draw(mem, primitive, vertices, admission, true)
    }

    /// Replace the vertex loop of a pushed primitive. Keep the now-empty
    /// Begin paired with the guest's End so the original flag, line width
    /// and ownership cleanup still runs. On refusal the guest sees its
    /// original empty assembler and can publish its own vertices.
    pub fn try_native_open_pushed_draw<const N: usize>(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        vertices: &[[(u8, [f32; 4]); N]],
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        if !self.native_boundary_ready(mem)? || !self.immediate.empty_begin(primitive)
            || !register_vertices_valid(vertices)
        {
            return Ok(false);
        }

        let mut scratch = self.immediate.clone();
        write_register_vertices(&mut scratch, vertices)?;
        let draw = scratch.end(0)?;
        let words = self.pushed_state(mem)?;
        let parts = self.immediate_parts(mem, "inline Begin/End", draw, words)?;
        scratch.begin(primitive, 0)?;

        let original = std::mem::replace(&mut self.immediate, scratch);
        let result = self.try_native_producer(mem, parts, admission);
        if !matches!(result, Ok(true)) {
            self.immediate = original;
        }
        result
    }

    fn try_native_register_draw<const N: usize>(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        vertices: &[[(u8, [f32; 4]); N]],
        admission: &dyn crate::DrawAdmission,
        pushed: bool,
    ) -> Result<bool, D3dError> {
        let entry = if pushed { "inline Begin/End" } else { "D3DDevice_End" };
        if !self.native_producer_ready(mem)? {
            return Ok(false);
        }
        if !register_vertices_valid(vertices) {
            return Ok(false);
        }
        let words = if pushed { self.pushed_state(mem)? } else { Arc::new(self.state(mem)?) };
        let initial = *self.immediate.registers();
        let mut scratch = Immediate::with_registers(initial);
        scratch.begin(primitive, 0)?;
        write_register_vertices(&mut scratch, vertices)?;
        let draw = scratch.end(0)?;
        let parts = self.immediate_parts(mem, entry, draw, words.clone())?;
        // The unwritten defaults must be resolved against the live carry.
        self.immediate = scratch;
        let result = self.try_native_producer(mem, parts, admission);
        if matches!(result, Ok(true)) {
            if !pushed {
                self.pushed = Some(words);
            }
        } else {
            self.immediate = Immediate::with_registers(initial);
        }
        result
    }
}

fn register_vertices_valid<const N: usize>(vertices: &[[(u8, [f32; 4]); N]]) -> bool {
    !vertices.is_empty() && vertices.iter().all(|vertex| {
        vertex.split_last().is_some_and(|(&(reg, _), attrs)| {
            reg == 0 && attrs.iter().all(|&(reg, _)| (1..16).contains(&reg))
        })
    })
}

fn write_register_vertices<const N: usize>(
    immediate: &mut Immediate,
    vertices: &[[(u8, [f32; 4]); N]],
) -> Result<(), PushError> {
    for vertex in vertices {
        for &(reg, value) in vertex {
            immediate.set(0, usize::from(reg), &value, 0, true)?;
        }
    }
    Ok(())
}

impl<B: Backend> Device<B> {
    /// Whether the current post transaction owns the next strip of this
    /// exact call. The preceding state-commit entry has pinned the cookie.
    pub fn owns_post_strip(&self, mem: &dyn GuestMemory, entry: u32, esp: u32, end: u32) -> bool {
        self.swallowing.as_ref().is_some_and(|s| {
            s.plan.drop && s.plan.call.is_some_and(|call| call.entry == entry && call.esp == esp)
                && s.return_cookie.is_some_and(|cookie| mem.read_u32(esp).ok() == Some(cookie))
                && s.plan.end_returns.get(s.next) == Some(&Some(end))
        })
    }

    /// Finish a screen strip whose BEGIN the guest already published. The
    /// assembled original is held or replayed by the normal post machinery.
    /// Before the candidate is issued, failure leaves the open BEGIN for
    /// the guest's vertex loop and End.
    pub fn try_post_strip(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: u32,
        esp: u32,
        end: u32,
        vertices: &[[[f32; 4]; 5]],
    ) -> Result<bool, D3dError> {
        if !self.owns_post_strip(mem, entry, esp, end)
            || !self.drain_admission_boundary(mem)?
            || !self.owns_post_strip(mem, entry, esp, end)
            || self.vertex_shader & 1 == 0
            || !self.immediate.is_open()
            || self.immediate.vertex_count() != 0
        {
            return Ok(false);
        }
        let mut scratch = self.immediate.clone();
        for vertex in vertices {
            for reg in [1, 2, 3, 4, 0] {
                scratch.set(0, reg, &vertex[reg], 0, true)?;
            }
        }
        let draw = scratch.end(0)?;
        let s = self.swallowing.as_mut().expect("post strip checked above");
        if s.plan.shapes.get(s.next) != Some(&(draw.primitive, draw.vertices.len()))
            || draw.primitive != PrimitiveType::TriangleStrip
            || draw.written != 0x1f
        {
            return Ok(false);
        }
        s.end_return = Some(end);
        self.immediate = scratch;
        self.swallow_immediate(mem, "D3DDevice_End", draw, None)?;
        Ok(true)
    }
}
