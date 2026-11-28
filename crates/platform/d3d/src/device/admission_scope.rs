//! A producer boundary drain that leaves interrupt callbacks queued.
use super::*;
impl<B: Backend> Device<B> {
    /// Finish work issued under the previous UI layout before changing the
    /// placement on subsequent packets. A deferred guest callback keeps
    /// the pending draw and falls back to the ordinary window transform.
    pub fn set_ui_fit(
        &mut self,
        mem: &mut dyn GuestMemory,
        fit: Option<crate::backend::UiFit>,
    ) -> Result<(), D3dError> {
        if self.ui_fit != fit {
            // Draining still needs the old fit on any pending draws.
            let drained = self.drain_admission_boundary(mem);
            self.ui_fit = None;
            if drained? {
                self.ui_fit = fit;
            }
        }
        Ok(())
    }

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

    /// The UI layout the draws at this point in the frame carry
    /// ([`Self::set_ui_fit`], `render_bridge::ui_layout`'s rule): `None`
    /// is the identity, the full guest window (a 4:3 output, or a
    /// background's full view).
    pub fn ui_fit(&self) -> Option<crate::backend::UiFit> {
        self.ui_fit
    }
}

impl<B: Backend> Device<B> {
    pub(super) fn native_producer_ready(&mut self, mem: &mut dyn GuestMemory) -> Result<bool, D3dError> {
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
    pub(super) fn try_native_producer(
        &mut self,
        mem: &mut dyn GuestMemory,
        mut parts: DrawParts,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<bool, D3dError> {
        let mut streams = std::mem::take(&mut parts.streams);
        self.native_refusal = None;
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
        self.native_refusal = None;
        if !self.native_producer_ready(mem)? {
            self.native_refusal = Some(NativeRefusal::Boundary);
            return Ok(false);
        }
        if count == 0 {
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
        self.try_native_indexed_draw_why(mem, primitive, count, indices, admission).map(|r| r.is_ok())
    }

    /// Why the last `try_native_*` draw that returned `false` did not
    /// commit, when it got as far as a step that names one.
    pub fn last_native_refusal(&self) -> Option<NativeRefusal> {
        self.native_refusal
    }

    /// [`Self::try_native_indexed_draw`] naming the refusal: the first
    /// step the draw did not pass.
    pub fn try_native_indexed_draw_why(
        &mut self,
        mem: &mut dyn GuestMemory,
        primitive: PrimitiveType,
        count: u32,
        indices: u32,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<Result<(), NativeRefusal>, D3dError> {
        if !self.native_producer_ready(mem)? {
            return Ok(Err(NativeRefusal::Boundary));
        }
        let Some(parts) = self.indexed_parts(mem, primitive, count, indices, None, false)? else {
            return Ok(Err(NativeRefusal::Geometry));
        };
        let words = parts.words.clone();
        let committed = self.try_native_producer(mem, parts, admission)?;
        if committed {
            self.pushed = Some(words);
            return Ok(Ok(()));
        }
        Ok(Err(self.native_refusal.take().unwrap_or(NativeRefusal::Backend)))
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
        let layout = VertexLayout {
            streams: vec![StreamLayout { stream: 0, attributes, stride: size }],
            pretransformed: false,
        };
        // The same state as the run's last draw: its resolution stands,
        // with these vertices (`inline_run`).
        if self.inline_runs {
            match self.try_inline_run(mem, &data, &layout, vertex.clone(), &words, admission) {
                Ok(super::inline_run::Fast::Full) => {}
                Ok(super::inline_run::Fast::Drawn) => return Ok(true),
                Ok(super::inline_run::Fast::Refused) => {
                    self.immediate = Immediate::with_registers(initial);
                    return Ok(false);
                }
                Err(e) => {
                    self.immediate = Immediate::with_registers(initial);
                    return Err(e);
                }
            }
        }
        let data_len = data.len();
        let parts = DrawParts {
            entry: ENTRY,
            indexed_esp: None,
            primitive,
            vertex_count,
            layout,
            vertex,
            streams: vec![StreamBytes { data: data.into(), stride: size }],
            indices: None,
            words,
        };
        self.inline_capture = self.inline_runs;
        self.stats.native_inline_full += 1;
        timeline::phase::count(timeline::phase::Counter::NativeInlineFull, 1);
        let result = self.try_native_producer(mem, parts, admission);
        self.inline_capture = false;
        if matches!(result, Ok(true)) {
            self.open_inline_run(data_len);
        } else {
            self.inline_run = None;
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
