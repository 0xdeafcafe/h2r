//! Ordered native commands share the pending draw list when they can use
//! its attachments. Opaque bundles keep their own run.
use super::{GfxBackend, PendingPass, SurfaceId};
use crate::backend::{ExternalBundles, ExternalDraws, ExternalEncoded, TargetCopy};
use std::sync::atomic::{AtomicU64, Ordering};
use timeline::native::{Metric, Stage, Timer};

pub(super) struct Run {
    key: (u64, SurfaceId, SurfaceId),
    copy: TargetCopy,
    stage: Stage,
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    stencil: bool,
    draws: Vec<ExternalBundles>,
}

static PASSES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn metric(copy: TargetCopy) -> Metric {
    match copy {
        TargetCopy::Reference => Metric::ReferencePasses,
        TargetCopy::Display => Metric::DisplayPasses,
    }
}

impl PendingPass {
    pub(super) fn new(color: SurfaceId, depth: Option<SurfaceId>) -> Self {
        Self {
            color,
            depth,
            clear: gfx::Clear::NONE,
            draws: Vec::new(),
            encoded: 1,
            tags: Vec::new(),
            native: Vec::new(),
        }
    }

    pub(super) fn count_native_passes(&self) {
        if self.native.is_empty() || !timeline::phase::enabled() {
            return;
        }
        let mut native = self.native.iter().peekable();
        let mut seen = 0u64;
        for (i, draw) in self.draws.iter().enumerate() {
            if draw.starts_pass() || draw.counts_samples() {
                seen = 0;
            }
            let Some((range, stage, copy)) = native.peek().copied() else {
                break;
            };
            if range.contains(&i) {
                let bit = 1u64 << (*stage as usize * 2 + usize::from(*copy == TargetCopy::Display));
                if seen & bit == 0 {
                    stage.count(metric(*copy), 1);
                    seen |= bit;
                }
                if range.end == i + 1 {
                    native.next();
                }
            }
        }
    }
}

impl GfxBackend {
    pub fn native_bundle_passes() -> [u64; 2] {
        std::array::from_fn(|i| PASSES[i].load(Ordering::Relaxed))
    }

    pub(super) fn encode_before_external(&mut self) -> bool {
        // A commit has no result channel. Preserve a fatal backend error for
        // the next operation with one, as the threaded backend does.
        if let Err(error) = self.encode_pending() {
            self.external_ordering_error = Some(error);
            return false;
        }
        true
    }

    pub(super) fn append_native(
        &mut self,
        stage: Stage,
        copy: TargetCopy,
        targets: (SurfaceId, SurfaceId),
        color: wgpu::TextureView,
        depth: wgpu::TextureView,
        stencil: bool,
        draws: ExternalBundles,
    ) {
        let key = (draws.run, targets.0, targets.1);
        if let Some(run) = &mut self.native_run
            && (run.key, run.copy, run.stage) == (key, copy, stage)
            && run.color == color
            && run.depth == depth
            && run.stencil == stencil
        {
            run.draws.push(draws);
            return;
        }
        self.finish_native_run();
        self.native_run = Some(Run {
            key,
            copy,
            stage,
            color,
            depth,
            stencil,
            draws: vec![draws],
        });
    }

    pub(super) fn append_native_draws(
        &mut self,
        stage: Stage,
        copy: TargetCopy,
        (color, depth): (SurfaceId, SurfaceId),
        mut draws: ExternalDraws,
        uploads: &gfx::ring::SubmissionUploads,
    ) -> bool {
        self.finish_native_run();
        let writes = std::mem::take(&mut draws.writes);
        if !writes.is_empty() && !self.write_ahead(uploads, writes) {
            return false;
        }
        let depth = (depth != color).then_some(depth);
        if self.pending.as_ref().is_some_and(|p| (p.color, p.depth) != (color, depth))
            && !self.encode_before_external()
        {
            return false;
        }
        let _encode = Timer::start(stage, Metric::EncodeNs);
        let start = self.pending.as_ref().map_or(0, |p| p.draws.len());
        self.frame_draw += 1;
        for (i, packet) in draws.packets.into_iter().enumerate() {
            self.append_recorded(color, depth, packet, i == 0, draws.discards);
        }
        let pending = self.pending.as_mut().expect("at least one validated draw");
        pending.native.push((start..pending.draws.len(), stage, copy));
        true
    }

    /// A stage's own targets' passes and copies, in the frame's open
    /// encoder after the pending pass: no command buffer of its own.
    pub(super) fn append_native_encoded(&mut self, stage: Stage, copy: TargetCopy, encoded: ExternalEncoded) -> bool {
        if !self.encode_before_external() {
            return false;
        }
        let _encode = Timer::start(stage, Metric::EncodeNs);
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        (encoded.commands)(encoder);
        *passes += encoded.passes;
        self.stats.render_passes += encoded.passes as u64;
        stage.count(metric(copy), encoded.passes as u64);
        true
    }

    /// The persistent ranges a packet run reads ([`ExternalDraws::writes`]).
    /// At the front of the submission while it holds no other bytes for them
    /// and no command recorded so far writes them in draw order. Otherwise
    /// copied in draw order after the pending pass, and the run starts a
    /// pass of its own (two views of the world in one submission).
    fn write_ahead(&mut self, uploads: &gfx::ring::SubmissionUploads, writes: Vec<(wgpu::Buffer, u64, Vec<u8>)>) -> bool {
        let arenas = &self.submitted_uploads;
        let ordered = |b: &wgpu::Buffer| arenas.iter().chain(std::iter::once(uploads)).any(|u| u.wrote_in_order(b));
        if fits_front(&self.front_writes, ordered, &writes) {
            for (buffer, offset, bytes) in writes {
                let held = self.front_writes.entry(buffer.clone()).or_default();
                if held.iter().any(|(o, d)| *o == offset && *d == bytes) {
                    continue;
                }
                uploads.write_front(&self.ctx.device, &self.ctx.queue, &buffer, offset, &bytes);
                held.push((offset, bytes));
            }
            return true;
        }
        if !self.encode_before_external() {
            return false;
        }
        timeline::phase::count(timeline::phase::Counter::OrderedWrites, 1);
        let (encoder, _) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        let mut ordered = gfx::ring::EncoderUploads { arena: uploads, device: &self.ctx.device, queue: &self.ctx.queue, encoder };
        for (buffer, offset, bytes) in &writes {
            ordered.write(buffer, *offset, bytes);
        }
        true
    }

    pub(super) fn finish_native_run(&mut self) {
        let Some(run) = self.native_run.take() else { return };
        let _encode = Timer::start(run.stage, Metric::EncodeNs);
        let (encoder, passes) = self.open.get_or_insert_with(||
            (self.ctx.device.create_command_encoder(&Default::default()), 0));
        ExternalBundles::encode_run(encoder, &run.color, &run.depth, run.stencil, &run.draws);
        *passes += 1;
        self.stats.render_passes += 1;
        run.stage.count(metric(run.copy), 1);
        if run.stage == Stage::Lights {
            PASSES[usize::from(run.copy == TargetCopy::Display)].fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Whether `writes` can go at the front of the submission: no command
/// writes their buffers in draw order (`ordered`) and no range overlaps
/// other bytes `held` there or earlier in `writes`.
#[allow(clippy::mutable_key_type, reason = "wgpu hashes and compares buffers by stable handle identity")]
fn fits_front(
    held: &std::collections::HashMap<wgpu::Buffer, Vec<(u64, Vec<u8>)>>,
    ordered: impl Fn(&wgpu::Buffer) -> bool,
    writes: &[(wgpu::Buffer, u64, Vec<u8>)],
) -> bool {
    let clash = |(o, d): (u64, &[u8]), (p, e): (u64, &[u8])| {
        o < p + e.len() as u64 && p < o + d.len() as u64 && (o, d) != (p, e)
    };
    writes.iter().enumerate().all(|(i, (buffer, offset, bytes))| {
        let mine = (*offset, bytes.as_slice());
        !ordered(buffer)
            && held.get(buffer).is_none_or(|held| held.iter().all(|(o, d)| !clash(mine, (*o, d))))
            && writes[..i].iter().all(|(b, o, d)| b != buffer || !clash(mine, (*o, d)))
    })
}

#[cfg(test)]
mod tests {
    use super::fits_front;

    #[test]
    fn front_writes_keep_one_content_per_range_and_yield_to_ordered_writes() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let buffer = || ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: 256, usage: wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let (a, b) = (buffer(), buffer());
        #[allow(clippy::mutable_key_type, reason = "wgpu hashes and compares buffers by stable handle identity")]
        let mut held = std::collections::HashMap::new();
        held.insert(a.clone(), vec![(0, vec![1; 16])]);
        let none = |_: &wgpu::Buffer| false;
        // The same bytes again, or other ranges and buffers.
        assert!(fits_front(&held, none, &[(a.clone(), 0, vec![1; 16]), (a.clone(), 16, vec![2; 16]), (b.clone(), 0, vec![3; 16])]));
        // Other bytes over a held range, even partly, or earlier in the run.
        assert!(!fits_front(&held, none, &[(a.clone(), 8, vec![1; 16])]));
        assert!(!fits_front(&held, none, &[(a.clone(), 0, vec![9; 16])]));
        assert!(!fits_front(&held, none, &[(b.clone(), 0, vec![3; 16]), (b.clone(), 4, vec![4; 4])]));
        // A buffer written in draw order this submission.
        assert!(!fits_front(&held, |x: &wgpu::Buffer| *x == b, &[(b.clone(), 64, vec![5; 16])]));
        assert!(fits_front(&held, |x: &wgpu::Buffer| *x == b, &[(a.clone(), 64, vec![5; 16])]));
    }
}
