//! Runs of native inline draws of one state (the particles): the first
//! resolves in full through `submit_inner`, and each next one whose device
//! state matches the run's reuses that resolution, with only its vertices
//! and carried defaults new (docs/graphics/native-particles.md,
//! "Same-state particles").
use super::*;

/// What a native inline draw's resolution depends on, beyond its own
/// vertices and carried defaults. Equal keys mean `submit_inner` would
/// resolve the same draw state, targets, textures and binding.
#[derive(PartialEq)]
pub(super) struct InlineKey {
    /// Push-buffer words decoded: any pushed state, texture, constant or
    /// target change.
    push_words: u64,
    /// D3D API entries ([`Device::live`]): any setter, present or clear.
    api_calls: u64,
    constants_generation: u64,
    rtt_epoch: u64,
    textures: [u32; 4],
    palettes: [u32; 4],
    render_target: u32,
    depth_stencil: u32,
    vertex_shader: u32,
    pixel_shader: u32,
    program_start: u32,
    viewport: Viewport,
    hw: HwState,
    ui_fit: Option<crate::backend::UiFit>,
    projection: (u32, u32),
    data_len: usize,
}

/// The run's first draw as it was resolved, admitted and queued.
pub(super) struct InlineRun<S> {
    /// Set once the opening draw has committed ([`Device::open_inline_run`]).
    pub(super) key: Option<InlineKey>,
    pub(super) words: Arc<StateWords>,
    /// Its shader inputs before the display projection changed them.
    pub(super) inputs: ShaderInputs,
    pub(super) draw: ResolvedDraw,
    pub(super) color: S,
    pub(super) depth: Option<S>,
    pub(super) carry_depth: Option<S>,
    pub(super) surfaces: crate::backend::ExternalSurfaces<S>,
    pub(super) request: crate::backend::ExternalPassRequest,
    pub(super) stage: timeline::native::Stage,
    pub(super) view: u32,
    pub(super) sampled: [bool; 4],
    pub(super) keys: [Option<TextureKey>; 4],
    pub(super) writes_color: bool,
    pub(super) writes_depth: bool,
    /// The opening draw's admission was retained for a replay's carry.
    pub(super) retains: bool,
}

/// A run's answer for the next inline draw.
pub(super) enum Fast {
    /// Queued on the fast path.
    Drawn,
    /// The producer refused the draw; the full path would too.
    Refused,
    /// The state differs or the queue declined: resolve in full.
    Full,
}

impl<B: Backend> Device<B> {
    /// Off: every native inline draw resolves in full (tests compare the
    /// two paths).
    pub fn set_inline_runs(&mut self, on: bool) {
        self.inline_runs = on;
        if !on {
            self.inline_run = None;
        }
    }

    fn inline_key(&self, data_len: usize) -> InlineKey {
        InlineKey {
            push_words: self.stats.push_words,
            api_calls: self.api_calls.load(AtomicOrdering::Relaxed),
            constants_generation: self.constants_generation,
            rtt_epoch: self.rtt_epoch,
            textures: self.textures,
            palettes: self.palettes,
            render_target: self.render_target,
            depth_stencil: self.depth_stencil,
            vertex_shader: self.vertex_shader,
            pixel_shader: self.pixel_shader,
            program_start: self.program_start,
            viewport: self.viewport,
            hw: self.hw,
            ui_fit: self.ui_fit,
            projection: (self.projection_factor().to_bits(), self.projection.camera_factor_bits()),
            data_len,
        }
    }

    /// The inline draw just committed through `submit_inner`: the run it
    /// opened (if it could) starts at the state now.
    pub(super) fn open_inline_run(&mut self, data_len: usize) {
        let key = self.inline_key(data_len);
        if let Some(run) = self.inline_run.as_mut() {
            run.key = Some(key);
        }
    }

    /// The next inline draw on the run: the run's resolved draw with these
    /// vertices and the live carried defaults, offered to `admission` and
    /// queued on the run's binding. Each check `submit_inner` would make
    /// on a changed state is one the key or the shader inputs fail first.
    pub(super) fn try_inline_run(
        &mut self,
        mem: &mut dyn GuestMemory,
        data: &[u8],
        layout: &VertexLayout,
        vertex: VertexProcessing,
        words: &Arc<StateWords>,
        admission: &dyn crate::DrawAdmission,
    ) -> Result<Fast, D3dError> {
        let Some(run) = self.inline_run.take() else { return Ok(Fast::Full) };
        if run.key.as_ref() != Some(&self.inline_key(data.len()))
            || !(Arc::ptr_eq(&run.words, words) || *run.words == **words)
            || run.draw.layout != *layout
        {
            return Ok(Fast::Full);
        }
        let inputs = self.shader_inputs(mem, vertex, layout, words)?;
        let mut expected = run.inputs.clone();
        expected.vertex_defaults = inputs.vertex_defaults;
        if expected != inputs {
            return Ok(Fast::Full);
        }
        let mut draw = run.draw.clone();
        draw.streams = vec![StreamBytes { data: data.to_vec().into(), stride: layout.streams[0].stride }];
        draw.shader.vertex_defaults = inputs.vertex_defaults;
        draw.raw = Arc::clone(words);
        let snapshot = crate::DrawSnapshot {
            draw: &draw,
            vertex_shader: self.vertex_shader,
            pixel_shader: self.pixel_shader,
            bound_textures: self.textures,
            sampled_stages: run.sampled,
            palettes: self.palettes,
            color_header: self.render_target,
            depth_header: self.depth_stencil,
        };
        let Some((plan, pass)) = crate::backend::admit_draw(admission, &snapshot) else {
            self.native_refusal = Some(NativeRefusal::Admission);
            return Ok(Fast::Refused);
        };
        let (Some(fallback), true) = (pass.resolved_fallback(), plan.stage == run.stage && plan.view == run.view) else {
            return Ok(Fast::Full);
        };
        if pass.indexed_tail().is_some() || pass.requires_independent_samples() || pass.counts_samples() {
            return Ok(Fast::Full);
        }
        let packets = if run.retains { pass.resolved_packets() } else { Vec::new() };
        let pass = Arc::new(std::sync::Mutex::new(pass));
        let retained = (!packets.is_empty()).then(|| crate::backend::RetainedAdmission {
            packets: packets.into(),
            surfaces: run.surfaces.clone(),
            request: run.request,
            plan: plan.clone(),
            pass: Arc::clone(&pass),
        });
        let (color, carry_depth) = (run.color, run.carry_depth);
        let carry = retained.as_ref().is_some_and(|a| self.backend.begin_native_carry(color, carry_depth, a));
        let stats = Arc::clone(&self.queued_external_stats);
        let queued = crate::backend::QueuedExternalDraw {
            surfaces: run.surfaces.clone(),
            request: run.request,
            plan,
            pass,
            fallback,
            fallback_depth: run.depth,
            finished: Box::new(move |fallback| {
                let count = if fallback.is_some() { &stats.dropped } else { &stats.committed };
                count.fetch_add(1, AtomicOrdering::Relaxed);
            }),
        };
        let textures: [Option<&B::Texture>; 4] =
            std::array::from_fn(|i| run.keys[i].and_then(|k| self.cache.get(&k)).map(|c| &c.host));
        if !self.backend.queue_external_draw(queued, textures) {
            if carry {
                self.backend.end_native_carry();
            }
            return Ok(Fast::Full);
        }
        if carry {
            self.backend.record_native_carry(color, carry_depth, textures);
            self.backend.end_native_carry();
        }
        if run.writes_color {
            self.color_surface_written(color);
            self.mark_color_written(self.render_target);
        }
        if run.writes_depth {
            self.mark_depth_written();
        }
        self.stats.native_inline_fast += 1;
        timeline::phase::count(timeline::phase::Counter::NativeInlineFast, 1);
        // Only this draw's own writes came after the key was checked.
        let mut run = run;
        run.key = Some(self.inline_key(data.len()));
        self.inline_run = Some(run);
        Ok(Fast::Drawn)
    }
}
