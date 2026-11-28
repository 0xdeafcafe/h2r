//! Machine snapshots (docs/runtime/snapshots.md, phase 4): the `d3d`
//! section.
//!
//! [`Device::save_snapshot`] lists every device field and gives each a
//! decision, without `..`, so a field added later does not compile until
//! someone decides it:
//!
//! * **Save**: bindings, vertex program and constants, viewport,
//!   transforms, scissors, the immediate-mode registers, push-buffer
//!   cursors, fence and swap count, the pushed state snapshot, NV2A
//!   register state, display state, visibility results (read first), the
//!   swap ring's order and the destroys the guest has yet to run.
//! * **Check**: what CreateDevice built from the same image and switches:
//!   the frame-buffer and depth headers, the ring's buffers and size.
//! * **Drop**: caches keyed by guest address (textures, vertex and index
//!   snapshots, translation and draw memos), render-to-texture targets and
//!   the alias mirror after their pixels are written back, depth copies
//!   and aliases. They refill from guest RAM on use.
//! * **Keep**: the loading device's backend, configuration, switches,
//!   tracker, counters and events.
//! * **Quiesce**: a `Begin` with no `End`, a push-buffer callback being
//!   delivered or waiting, a latched fault, an occlusion grid asked for, a
//!   visibility test still counting, a `PersistDisplay` surface.
//!
//! Host pixels are not saved: the swap ring and the depth buffer keep the
//! loading device's pixels, and surfaces of guest surface headers that are
//! not texture levels are made again blank. Draws read the same guest
//! state either way, so the draw stream after a load is the saving run's;
//! the first frame's history (Halo samples last frame's buffers) is not.
//!
//! The frame wait and vblank delivery ([`crate::vblank`]) keep their Rust
//! state in `ExecCtx::call_guest` continuations, which
//! `kernel::Machine::quiescent` already reports as blockers; the device
//! itself holds none of it. So a D3D save is allowed whenever the machine
//! is quiescent and [`Device::snapshot_blockers`] is empty. A render
//! thread ([`crate::ThreadedBackend`]) must be drained (`finish`) by the
//! caller first.
use super::visibility::{VisibilityResult, VisibilityTest};
use super::*;
use crate::layout::{STAGE_SLOTS, STAGES};
use crate::pushbuf::ImmediateVertex;
use crate::regs::PAGE;
use ::snapshot::codec::{Reader, Writer};
use ::snapshot::{Codec, Snapshot, SnapshotWriter, Tag};

/// The section's tag.
pub const SECTION: Tag = Tag::new("d3d");
const DEVICE_VERSION: u32 = 1;
const FILM_VERSION: u32 = 2;
/// Most entries any saved list may claim (scissors are at most 8, the
/// rest are bounded by the guest's index and header spaces).
const MAX_ITEMS: usize = 1 << 16;

/// Why a save or load did not happen.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// Something is in flight that a snapshot cannot carry.
    #[error("d3d is not quiescent: {}", .0.join("; "))]
    Busy(Vec<String>),
    /// The section is damaged or missing.
    #[error(transparent)]
    Codec(#[from] ::snapshot::Error),
    /// The snapshot was saved on a device created differently.
    #[error("d3d snapshot does not fit this device: {0}")]
    Mismatch(String),
    /// Draining or writing back failed.
    #[error(transparent)]
    D3d(#[from] D3dError),
}

fn corrupt(what: &str) -> ::snapshot::Error {
    ::snapshot::Error::Corrupt("d3d", what.to_owned())
}

fn put_words(w: &mut Writer, words: &[u32]) {
    for v in words {
        w.u32(*v);
    }
}

fn get_words(r: &mut Reader<'_>, out: &mut [u32]) -> Result<(), ::snapshot::Error> {
    for v in out {
        *v = r.u32()?;
    }
    Ok(())
}

fn put_f32s(w: &mut Writer, v: &[f32]) {
    for f in v {
        w.u32(f.to_bits());
    }
}

fn get_f32s(r: &mut Reader<'_>, out: &mut [f32]) -> Result<(), ::snapshot::Error> {
    for f in out {
        *f = f32::from_bits(r.u32()?);
    }
    Ok(())
}

fn put_u32s(w: &mut Writer, v: &[u32]) {
    w.list(v, |w, x| {
        w.u32(*x);
    });
}

fn get_u32s(r: &mut Reader<'_>) -> Result<Vec<u32>, ::snapshot::Error> {
    let v = r.list(4, |r| r.u32())?;
    if v.len() > MAX_ITEMS {
        return Err(corrupt("list too long"));
    }
    Ok(v)
}

fn put_opt(w: &mut Writer, v: Option<u32>) {
    w.bool(v.is_some()).u32(v.unwrap_or(0));
}

fn get_opt(r: &mut Reader<'_>) -> Result<Option<u32>, ::snapshot::Error> {
    let some = r.bool()?;
    let v = r.u32()?;
    Ok(some.then_some(v))
}

fn put_streams(w: &mut Writer, s: &[StreamSource]) {
    w.list(s, |w, s| {
        w.u32(s.buffer).u32(s.stride).u32(s.offset);
    });
}

fn get_streams(r: &mut Reader<'_>) -> Result<Vec<StreamSource>, ::snapshot::Error> {
    let v = r.list(12, |r| {
        Ok(StreamSource {
            buffer: r.u32()?,
            stride: r.u32()?,
            offset: r.u32()?,
        })
    })?;
    if v.len() > 16 {
        return Err(corrupt("more than 16 streams"));
    }
    Ok(v)
}

fn put_hw(w: &mut Writer, hw: &HwState) {
    let HwState {
        control0,
        viewport_offset,
        viewport_scale,
        clip_min,
        clip_max,
        constant_load,
        program_load,
        program_start,
        execution_mode,
        cxt_write_enable,
        stipple_control,
        stipple_pattern,
        // A per-draw override, cleared before any snapshot.
        display_stipple: _,
        zstencil_clear_value,
        color_clear_value,
        vertex_format,
        vertex_offset,
        zpass_count_enable,
        eye_vector,
    } = hw;
    put_opt(w, *control0);
    put_f32s(w, viewport_offset);
    put_f32s(w, viewport_scale);
    put_f32s(w, &[*clip_min, *clip_max]);
    w.u32(*constant_load).u32(*program_load);
    put_opt(w, *program_start);
    w.u32(*execution_mode).u32(*cxt_write_enable);
    put_opt(w, *stipple_control);
    put_words(w, stipple_pattern);
    w.u32(*zstencil_clear_value).u32(*color_clear_value);
    put_words(w, vertex_format);
    put_words(w, vertex_offset);
    w.bool(*zpass_count_enable);
    put_f32s(w, eye_vector);
}

fn get_hw(r: &mut Reader<'_>) -> Result<HwState, ::snapshot::Error> {
    let mut hw = HwState {
        control0: get_opt(r)?,
        ..HwState::default()
    };
    get_f32s(r, &mut hw.viewport_offset)?;
    get_f32s(r, &mut hw.viewport_scale)?;
    hw.clip_min = f32::from_bits(r.u32()?);
    hw.clip_max = f32::from_bits(r.u32()?);
    hw.constant_load = r.u32()?;
    hw.program_load = r.u32()?;
    hw.program_start = get_opt(r)?;
    hw.execution_mode = r.u32()?;
    hw.cxt_write_enable = r.u32()?;
    hw.stipple_control = get_opt(r)?;
    get_words(r, &mut hw.stipple_pattern)?;
    hw.zstencil_clear_value = r.u32()?;
    hw.color_clear_value = r.u32()?;
    get_words(r, &mut hw.vertex_format)?;
    get_words(r, &mut hw.vertex_offset)?;
    hw.zpass_count_enable = r.bool()?;
    get_f32s(r, &mut hw.eye_vector)?;
    Ok(hw)
}

/// The section, decoded and checked before anything changes.
struct Saved {
    // Checked against the loading device.
    back_header: u32,
    frame_headers: Vec<u32>,
    depth_header: u32,
    /// Every frame buffer's `Data`, in the ring's order.
    ring: Vec<u32>,
    size: (u32, u32),
    flip: bool,
    // Saved. Surfaces are named by their buffer's `Data`.
    back: Vec<u32>,
    front: u32,
    shown: u32,
    copy_pending: bool,
    render_target: u32,
    depth_stencil: u32,
    textures: [u32; 4],
    palettes: [u32; 4],
    vertex_shader: u32,
    pixel_shader: u32,
    streams: [StreamSource; 16],
    stream_inputs: Option<Vec<StreamSource>>,
    program: Box<[u32; PROGRAM_SLOTS * 4]>,
    program_start: u32,
    constants: Box<[u32; CONSTANTS * 4]>,
    viewport: Viewport,
    transforms: Vec<(u32, [f32; 16])>,
    scissors: Vec<Rect>,
    scissor_exclusive: bool,
    immediate: ImmediateVertex,
    consumed: u32,
    pushed: Option<StateWords>,
    swap_count: u32,
    fence: u32,
    fence_at: u32,
    regs: Option<[Vec<u8>; 2]>,
    hw: HwState,
    flicker: Option<u32>,
    gamma: Option<Box<[u8; GAMMA_RAMP_BYTES]>>,
    vblanks: u32,
    swapped: bool,
    shown_lost: bool,
    pending_vblanks: Vec<VBlank>,
    /// Index, time, swap, count.
    visibility: Vec<(u32, u32, u32, u32)>,
    pending_destroys: Vec<u32>,
    torn_down: Vec<u32>,
}

impl Saved {
    fn put(&self, w: &mut Writer) {
        let Saved {
            back_header,
            frame_headers,
            depth_header,
            ring,
            size,
            flip,
            back,
            front,
            shown,
            copy_pending,
            render_target,
            depth_stencil,
            textures,
            palettes,
            vertex_shader,
            pixel_shader,
            streams,
            stream_inputs,
            program,
            program_start,
            constants,
            viewport,
            transforms,
            scissors,
            scissor_exclusive,
            immediate,
            consumed,
            pushed,
            swap_count,
            fence,
            fence_at,
            regs,
            hw,
            flicker,
            gamma,
            vblanks,
            swapped,
            shown_lost,
            pending_vblanks,
            visibility,
            pending_destroys,
            torn_down,
        } = self;
        w.u32(*back_header);
        put_u32s(w, frame_headers);
        w.u32(*depth_header);
        put_u32s(w, ring);
        w.u32(size.0).u32(size.1).bool(*flip);
        put_u32s(w, back);
        w.u32(*front).u32(*shown).bool(*copy_pending);
        w.u32(*render_target).u32(*depth_stencil);
        put_words(w, textures);
        put_words(w, palettes);
        w.u32(*vertex_shader).u32(*pixel_shader);
        put_streams(w, streams);
        w.bool(stream_inputs.is_some());
        put_streams(w, stream_inputs.as_deref().unwrap_or_default());
        put_words(w, &program[..]);
        w.u32(*program_start);
        put_words(w, &constants[..]);
        w.u32(viewport.x)
            .u32(viewport.y)
            .u32(viewport.width)
            .u32(viewport.height);
        put_f32s(w, &[viewport.min_z, viewport.max_z]);
        w.list(transforms, |w, (state, m)| {
            w.u32(*state);
            put_f32s(w, m);
        });
        w.list(scissors, |w, r| {
            put_words(w, &[r.x1, r.y1, r.x2, r.y2].map(|v| v as u32));
        });
        w.bool(*scissor_exclusive);
        for reg in immediate {
            put_f32s(w, reg);
        }
        w.u32(*consumed);
        w.bool(pushed.is_some());
        if let Some(p) = pushed {
            put_words(w, &p.render);
            for stage in &p.stages {
                put_words(w, stage);
            }
        }
        w.u32(*swap_count).u32(*fence).u32(*fence_at);
        w.bool(regs.is_some());
        if let Some([a, b]) = regs {
            w.raw(a).raw(b);
        }
        put_hw(w, hw);
        put_opt(w, *flicker);
        w.bool(gamma.is_some());
        if let Some(g) = gamma {
            w.raw(&g[..]);
        }
        w.u32(*vblanks).bool(*swapped).bool(*shown_lost);
        w.list(pending_vblanks, |w, v| {
            w.u32(v.callback);
            put_words(w, &v.record);
        });
        w.list(visibility, |w, &(index, time, swap, count)| {
            w.u32(index).u32(time).u32(swap).u32(count);
        });
        put_u32s(w, pending_destroys);
        put_u32s(w, torn_down);
    }

    fn get(bytes: &[u8]) -> Result<Self, ::snapshot::Error> {
        let r = &mut Reader::new(bytes, "d3d");
        let back_header = r.u32()?;
        let frame_headers = get_u32s(r)?;
        let depth_header = r.u32()?;
        let ring = get_u32s(r)?;
        let size = (r.u32()?, r.u32()?);
        let flip = r.bool()?;
        let back = get_u32s(r)?;
        let front = r.u32()?;
        let shown = r.u32()?;
        let copy_pending = r.bool()?;
        let render_target = r.u32()?;
        let depth_stencil = r.u32()?;
        let mut textures = [0; 4];
        get_words(r, &mut textures)?;
        let mut palettes = [0; 4];
        get_words(r, &mut palettes)?;
        let vertex_shader = r.u32()?;
        let pixel_shader = r.u32()?;
        let bound = get_streams(r)?;
        let mut streams = [StreamSource::default(); 16];
        if bound.len() != 16 {
            return Err(corrupt("stream table is not 16 entries"));
        }
        streams.copy_from_slice(&bound);
        let has_inputs = r.bool()?;
        let inputs = get_streams(r)?;
        let stream_inputs = has_inputs.then_some(inputs);
        let mut program = Box::new([0; PROGRAM_SLOTS * 4]);
        get_words(r, &mut program[..])?;
        let program_start = r.u32()?;
        let mut constants = Box::new([0; CONSTANTS * 4]);
        get_words(r, &mut constants[..])?;
        let viewport = Viewport {
            x: r.u32()?,
            y: r.u32()?,
            width: r.u32()?,
            height: r.u32()?,
            min_z: f32::from_bits(r.u32()?),
            max_z: f32::from_bits(r.u32()?),
        };
        let transforms = r.list(4 + 64, |r| {
            let state = r.u32()?;
            let mut m = [0.0; 16];
            get_f32s(r, &mut m)?;
            Ok((state, m))
        })?;
        let scissors = r.list(16, |r| {
            Ok(Rect {
                x1: r.u32()? as i32,
                y1: r.u32()? as i32,
                x2: r.u32()? as i32,
                y2: r.u32()? as i32,
            })
        })?;
        if transforms.len() > MAX_ITEMS || scissors.len() > MAX_SCISSORS as usize {
            return Err(corrupt("too many transforms or scissors"));
        }
        let scissor_exclusive = r.bool()?;
        let mut immediate = [[0.0; 4]; crate::pushbuf::REGISTERS];
        for reg in &mut immediate {
            get_f32s(r, reg)?;
        }
        let consumed = r.u32()?;
        let pushed = if r.bool()? {
            let mut render = [0; RENDER_STATE_SLOTS];
            get_words(r, &mut render)?;
            let mut stages = [[0; STAGE_SLOTS]; STAGES];
            for stage in &mut stages {
                get_words(r, stage)?;
            }
            Some(StateWords { render, stages })
        } else {
            None
        };
        let swap_count = r.u32()?;
        let fence = r.u32()?;
        let fence_at = r.u32()?;
        let regs = if r.bool()? {
            let page = PAGE as usize;
            Some([r.raw(page)?.to_vec(), r.raw(page)?.to_vec()])
        } else {
            None
        };
        let hw = get_hw(r)?;
        let flicker = get_opt(r)?;
        let gamma = if r.bool()? {
            Some(Box::new(r.array::<GAMMA_RAMP_BYTES>()?))
        } else {
            None
        };
        let vblanks = r.u32()?;
        let swapped = r.bool()?;
        let shown_lost = r.bool()?;
        let pending_vblanks = r.list(16, |r| {
            let callback = r.u32()?;
            let mut record = [0; 3];
            get_words(r, &mut record)?;
            Ok(VBlank { callback, record })
        })?;
        let visibility = r.list(16, |r| Ok((r.u32()?, r.u32()?, r.u32()?, r.u32()?)))?;
        if pending_vblanks.len() > MAX_ITEMS
            || visibility.len() > VISIBILITY_INDICES as usize
            || visibility.iter().any(|v| v.0 >= VISIBILITY_INDICES)
        {
            return Err(corrupt("vblank queue or visibility tests out of range"));
        }
        let pending_destroys = get_u32s(r)?;
        let torn_down = get_u32s(r)?;
        r.finish()?;
        Ok(Saved {
            back_header,
            frame_headers,
            depth_header,
            ring,
            size,
            flip,
            back,
            front,
            shown,
            copy_pending,
            render_target,
            depth_stencil,
            textures,
            palettes,
            vertex_shader,
            pixel_shader,
            streams,
            stream_inputs,
            program,
            program_start,
            constants,
            viewport,
            transforms,
            scissors,
            scissor_exclusive,
            immediate,
            consumed,
            pushed,
            swap_count,
            fence,
            fence_at,
            regs,
            hw,
            flicker,
            gamma,
            vblanks,
            swapped,
            shown_lost,
            pending_vblanks,
            visibility,
            pending_destroys,
            torn_down,
        })
    }
}

/// The `Data` of ring surface `s`.
fn data_of<S: Copy + Eq>(sw: &Swap<S>, s: S) -> Result<u32, SnapshotError> {
    sw.buffers
        .iter()
        .find(|b| b.1 == s)
        .map(|b| b.0)
        .ok_or_else(|| SnapshotError::Mismatch("a ring surface is not a frame buffer".into()))
}

/// The ring surface whose `Data` is `data`.
fn surface_of<S: Copy>(sw: &Swap<S>, data: u32) -> Result<S, SnapshotError> {
    sw.buffers
        .iter()
        .find(|b| b.0 == data)
        .map(|b| b.1)
        .ok_or_else(|| SnapshotError::Mismatch(format!("no frame buffer at {data:#010x}")))
}

impl<B: Backend> Device<B> {
    /// What keeps this device from being saved now, empty when nothing
    /// does. Checked between slices; the push buffer is drained by the
    /// save itself.
    pub fn snapshot_blockers(&self) -> Vec<String> {
        let mut why = self.film_load_blockers();
        if !self.created {
            why.push("no device created".to_string());
        }
        why
    }

    /// A film load may construct the device on a fresh backend after RAM
    /// restore. All in-flight-work blockers still apply.
    pub fn film_load_blockers(&self) -> Vec<String> {
        let mut why = Vec::new();
        if let Some(f) = &self.fault {
            why.push(format!("faulted: {f}"));
        }
        if self.immediate.is_open() {
            why.push("inside Begin/End".into());
        }
        if let Some(cb) = self.active_callback.or(self.undelivered) {
            why.push(format!(
                "push-buffer callback {:#010x}({:#x}) in delivery",
                cb.function, cb.context
            ));
        }
        if !self.pending_external.is_empty() {
            why.push("an external pass is asked for".into());
        }
        if self.scoped_admission.as_ref().is_some_and(|(s, _)| !s.closed.load(AtomicOrdering::Acquire)) {
            why.push("a native draw is scoped to a guest call".into());
        }
        if self.ui_fit.is_some() {
            why.push("a fitted UI draw scope is active".into());
        }
        if self.swallowing.is_some() {
            why.push("a swallow of planned draws is armed".into());
        }
        if self.hw.zpass_count_enable {
            why.push("a visibility test is counting".into());
        }
        if self.display.persisted.is_some() {
            why.push("PersistDisplay kept a surface".into());
        }
        why
    }

    /// Adds the `d3d` section. Call between slices, before guest RAM is
    /// saved: this drains a render thread (`Backend::drain`) and the push
    /// buffer, writes dirty render-to-texture levels and the alias mirror
    /// back into guest memory, and reads every visibility test's count, so
    /// the saving run and every load carry on from the same guest bytes
    /// and answers. Nothing else changes.
    pub fn save_snapshot(
        &mut self,
        mem: &mut dyn GuestMemory,
        out: &mut SnapshotWriter,
    ) -> Result<(), SnapshotError> {
        self.write_snapshot(mem, out, false)
    }

    /// A film keyframe also records the creation parameters so a new
    /// playback process can allocate the host surfaces before its first draw.
    pub fn save_film_snapshot(
        &mut self,
        mem: &mut dyn GuestMemory,
        out: &mut SnapshotWriter,
    ) -> Result<(), SnapshotError> {
        self.write_snapshot(mem, out, true)
    }

    fn write_snapshot(
        &mut self,
        mem: &mut dyn GuestMemory,
        out: &mut SnapshotWriter,
        film: bool,
    ) -> Result<(), SnapshotError> {
        let busy = self.snapshot_blockers();
        if !busy.is_empty() {
            return Err(SnapshotError::Busy(busy));
        }
        // A render thread runs what was queued before anything is read
        // back (`ThreadedBackend::finish`).
        self.backend.drain().map_err(D3dError::Backend)?;
        self.sync_push_buffer(mem)?;
        let busy = self.snapshot_blockers();
        if !busy.is_empty() {
            return Err(SnapshotError::Busy(busy));
        }
        self.flush_render_targets(mem)?;
        self.resolve_visibility()?;
        let mut w = Writer::new();
        let version = if film {
            let params = self.params.ok_or_else(|| {
                SnapshotError::Mismatch("missing present parameters".into())
            })?;
            put_words(&mut w, &params.words());
            FILM_VERSION
        } else {
            DEVICE_VERSION
        };
        self.saved()?.put(&mut w);
        out.section(SECTION, version, Codec::Zstd, w.into_bytes())?;
        Ok(())
    }

    /// Every ended visibility test's count, read from the backend (a wait).
    fn resolve_visibility(&mut self) -> Result<(), D3dError> {
        let mut tickets: Vec<(u32, u64)> = self
            .visibility
            .iter()
            .filter_map(|(i, t)| match t.result {
                VisibilityResult::Ticket(ticket) => Some((*i, ticket)),
                VisibilityResult::Count(_) => None,
            })
            .collect();
        tickets.sort_unstable();
        for (i, ticket) in tickets {
            let samples = self
                .backend
                .read_pixel_count(ticket)
                .map_err(D3dError::Backend)?;
            let count = u32::try_from(samples).unwrap_or(u32::MAX);
            if let Some(t) = self.visibility.get_mut(&i) {
                t.result = VisibilityResult::Count(count);
            }
        }
        Ok(())
    }

    /// The saved state, every field decided.
    fn saved(&self) -> Result<Saved, SnapshotError> {
        let Device {
            // Boot state: CreateDevice built it from the same image and
            // switches, and the loading device keeps its own.
            cfg: _,
            // A host hook the boot plan installs again.
            indexed_admission: _,
            // A per-draw reading of the last refusal, made again on use.
            native_refusal: _,
            // A cache of the last inline draw's resolution, keyed on the
            // live state; a load drops it.
            inline_run: _,
            inline_capture: _,
            // The supplied draw's pixel words, set and cleared around
            // one draw; never live at a save.
            supplied_pixel: _,
            api_calls: _,
            // Switch.
            inline_runs: _,
            translator: _,
            backend: _,
            created: _,
            params: _,
            depth: _,
            encode: _,
            // Checked against the loading device.
            back_header,
            frame_headers,
            depth_header,
            // The ring's order is saved by `Data`; its buffers are checked.
            swap,
            // Pixels on the host only: made again blank on use.
            other_surfaces: _,
            // Each made by role with its surface; given back with it.
            target_roles: _,
            render_target,
            depth_stencil,
            textures,
            palettes,
            vertex_shader,
            pixel_shader,
            streams,
            stream_inputs,
            // Caches and scratch (drop_host_caches).
            vertex_memo: _,
            world_vertex_plans: _,
            translated: _,
            stream_pool: _,
            stream_pool_bytes: _,
            stream_scratch: _,
            vertex_cache: _,
            index_cache: _,
            push_words: _,
            program,
            program_start,
            microcode_memo: _,
            microcode_seen: _,
            // Switches.
            draw_sharing: _,
            constants,
            // A cache generation; bumped by the drop.
            constants_generation: _,
            draw_constants: _,
            draw_pixel_shader: _,
            draw_combiner: _,
            viewport,
            transforms,
            scissors,
            scissor_exclusive,
            immediate,
            consumed,
            pushed,
            // Quiesce (snapshot_blockers).
            fault: _,
            swap_count,
            fence,
            fence_at,
            regs,
            cache: _,
            // Nothing held over it survives the drop.
            tracker: _,
            // Diagnostics.
            events: _,
            last_stop: _,
            stats: _,
            queued_external_stats: _,
            hw,
            // Quiesce.
            pending_external: _,
            ui_fit: _, // Host render callback scope.
            projection: _, // Derived from the current output and immutable GPU registers.
            present_cameras: _, // Host image metadata, outside the guest snapshot.
            admissions: _, // Host services, retained across guest loads.
            opaque_world_admission: _,
            opaque_targets: _,
            scoped_admission: _, // Quiesce the call before saving.
            residual: _,
            indexed_geometry: _,
            indexed_geometry_scope: _,
            swallowing: _,
            swallow_mismatch: _,
            display,
            // Written back by save_snapshot, then dropped by a load.
            rtt: _,
            rtt_epoch: _,
            texture_flushed: _,
            depth_generation: _,
            depth_color: _,
            depth_color_newer: _,
            depth_copy: _,
            depth_alias_decodes: _,
            depth_read_generation: _,
            depth_alias: _,
            frame_snaps: _,
            depth_level_tex: _,
            // Switches.
            lazy_depth: _,
            leave_read: _,
            rtt_synced: _,
            rtt_stores: _,
            // Switches.
            alias_mirror: _,
            mirror_depth: _,
            // Flushed by save_snapshot, dropped by a load.
            mirror: _,
            mirror_stale: _,
            // Switch.
            rtt_direct: _,
            // Quiesce.
            active_callback: _,
            undelivered: _,
            visibility,
            // Switches.
            visibility_poll: _,
            glyph_smoothing: _,
            pending_destroys,
            torn_down,
        } = self;
        let display::Display {
            flicker,
            gamma,
            // Quiesce.
            persisted: _,
            vblanks,
            swapped,
            shown_lost,
            // A host setting (set_vblanks_per_swap).
            per_swap: _,
            pending,
        } = display;
        let sw = swap
            .as_ref()
            .ok_or_else(|| SnapshotError::Busy(vec!["no swap ring".into()]))?;
        let back = sw
            .back
            .iter()
            .map(|s| data_of(sw, *s))
            .collect::<Result<Vec<_>, _>>()?;
        let mut visibility: Vec<(u32, u32, u32, u32)> = visibility
            .iter()
            .map(|(i, t)| match t.result {
                VisibilityResult::Count(c) => Ok((*i, t.time, t.swap, c)),
                VisibilityResult::Ticket(_) => Err(SnapshotError::Busy(vec![format!(
                    "visibility test {i:#x} is not read"
                )])),
            })
            .collect::<Result<_, _>>()?;
        visibility.sort_unstable();
        let mut transforms: Vec<(u32, [f32; 16])> =
            transforms.iter().map(|(k, v)| (*k, *v)).collect();
        transforms.sort_unstable_by_key(|t| t.0);
        let mut torn_down: Vec<u32> = torn_down.iter().copied().collect();
        torn_down.sort_unstable();
        Ok(Saved {
            back_header: *back_header,
            frame_headers: frame_headers.clone(),
            depth_header: *depth_header,
            ring: sw.buffers.iter().map(|b| b.0).collect(),
            size: sw.size,
            flip: sw.flip,
            back,
            front: data_of(sw, sw.front)?,
            shown: data_of(sw, sw.shown)?,
            copy_pending: sw.copy_pending,
            render_target: *render_target,
            depth_stencil: *depth_stencil,
            textures: *textures,
            palettes: *palettes,
            vertex_shader: *vertex_shader,
            pixel_shader: *pixel_shader,
            streams: *streams,
            stream_inputs: stream_inputs.clone(),
            program: program.clone(),
            program_start: *program_start,
            constants: constants.clone(),
            viewport: *viewport,
            transforms,
            scissors: scissors.clone(),
            scissor_exclusive: *scissor_exclusive,
            immediate: *immediate.registers(),
            consumed: *consumed,
            pushed: pushed.as_deref().cloned(),
            swap_count: *swap_count,
            fence: *fence,
            fence_at: *fence_at,
            regs: regs.as_ref().map(|r| r.shadow().clone()),
            hw: *hw,
            flicker: *flicker,
            gamma: gamma.clone(),
            vblanks: *vblanks,
            swapped: *swapped,
            shown_lost: *shown_lost,
            pending_vblanks: pending.clone(),
            visibility,
            pending_destroys: pending_destroys.clone(),
            torn_down,
        })
    }

    /// Replaces this device's state with a snapshot's. The device must be
    /// created the same way (the same image, switches and present
    /// parameters), and guest RAM already loaded: host caches are dropped
    /// and refill from it. Nothing changes if the section does not fit.
    pub fn load_snapshot(&mut self, snapshot: &Snapshot<'_>) -> Result<(), SnapshotError> {
        self.restore_device(snapshot, None)
    }

    /// Load in an isolated film player, where the guest has not executed
    /// CreateDevice. Only host surfaces and the encode table are rebuilt.
    /// Guest RAM already contains the device object and register pages.
    pub fn load_film_snapshot(
        &mut self,
        snapshot: &Snapshot<'_>,
        mem: &dyn GuestMemory,
    ) -> Result<(), SnapshotError> {
        self.restore_device(snapshot, Some(mem))
    }

    fn restore_device(
        &mut self,
        snapshot: &Snapshot<'_>,
        film_memory: Option<&dyn GuestMemory>,
    ) -> Result<(), SnapshotError> {
        let (version, bytes) = snapshot.section(SECTION)?
            .ok_or(::snapshot::Error::MissingSection(SECTION))?;
        let (params, s) = match version {
            DEVICE_VERSION => (None, Saved::get(&bytes)?),
            FILM_VERSION => {
                let mut r = Reader::new(&bytes, "d3d");
                let mut words = [0; 17];
                get_words(&mut r, &mut words)?;
                let remaining = r.remaining();
                let saved = Saved::get(r.raw(remaining)?)?;
                (Some(PresentParameters::from_words(words)), saved)
            }
            other => return Err(::snapshot::Error::SectionVersion(SECTION, other).into()),
        };
        if !self.created && let Some(mem) = film_memory {
            let p = params.ok_or_else(|| {
                SnapshotError::Mismatch("cold restore needs the recorded present parameters".into())
            })?;
            self.allocate_frame_buffers(mem, p)?;
            self.regs = self.cfg.nv2a_window
                .map(RegisterWindow::new)
                .transpose()
                .map_err(SnapshotError::Mismatch)?;
            self.params = Some(p);
            self.created = true;
        }
        if film_memory.is_some() && params.is_some() && params != self.params {
            return Err(SnapshotError::Mismatch("present parameters differ".into()));
        }
        self.check(&s)?;
        self.drop_host_caches()?;
        self.backend.reset_present_metadata().map_err(D3dError::Backend)?;
        self.present_cameras.clear();
        self.ui_fit = None;
        self.scoped_admission = None;
        self.apply(s)
    }

    fn check(&self, s: &Saved) -> Result<(), SnapshotError> {
        let mismatch = |what: &str| -> Result<(), SnapshotError> {
            Err(SnapshotError::Mismatch(what.to_string()))
        };
        if !self.created {
            return mismatch("no device created");
        }
        let Some(sw) = &self.swap else {
            return mismatch("no swap ring");
        };
        if s.back_header != self.back_header
            || s.frame_headers != self.frame_headers
            || s.depth_header != self.depth_header
        {
            return mismatch("frame-buffer headers differ");
        }
        let ring: Vec<u32> = sw.buffers.iter().map(|b| b.0).collect();
        if s.ring != ring || s.size != sw.size || s.flip != sw.flip {
            return mismatch("the swap ring differs (present parameters)");
        }
        if s.back.len() != sw.back.len() {
            return mismatch("back-buffer count differs");
        }
        for d in s.back.iter().chain([&s.front, &s.shown]) {
            surface_of(sw, *d)?;
        }
        if s.regs.is_some() != self.regs.is_some() {
            return mismatch("NV2A register window present on one side only");
        }
        Ok(())
    }

    fn apply(&mut self, s: Saved) -> Result<(), SnapshotError> {
        let (back, front, shown) = {
            let sw = self.swap.as_ref().expect("checked");
            let back = s
                .back
                .iter()
                .map(|d| surface_of(sw, *d))
                .collect::<Result<Vec<_>, _>>()?;
            (back, surface_of(sw, s.front)?, surface_of(sw, s.shown)?)
        };
        let sw = self.swap.as_mut().expect("checked");
        sw.back = back;
        sw.front = front;
        sw.shown = shown;
        sw.copy_pending = s.copy_pending;
        if let (Some(w), Some(shadow)) = (self.regs.as_mut(), s.regs) {
            w.restore_shadow(shadow).map_err(SnapshotError::Mismatch)?;
        }
        self.render_target = s.render_target;
        self.depth_stencil = s.depth_stencil;
        self.textures = s.textures;
        self.palettes = s.palettes;
        self.vertex_shader = s.vertex_shader;
        self.pixel_shader = s.pixel_shader;
        self.streams = s.streams;
        self.stream_inputs = s.stream_inputs;
        self.program = s.program;
        self.program_start = s.program_start;
        self.constants = s.constants;
        self.viewport = s.viewport;
        self.transforms = s.transforms.into_iter().collect();
        self.scissors = s.scissors;
        self.scissor_exclusive = s.scissor_exclusive;
        self.immediate = crate::pushbuf::Immediate::with_registers(s.immediate);
        self.consumed = s.consumed;
        self.pushed = s.pushed.map(Arc::new);
        self.inline_run = None;
        self.fault = None;
        self.swap_count = s.swap_count;
        self.fence = s.fence;
        self.fence_at = s.fence_at;
        self.hw = s.hw;
        self.display.flicker = s.flicker;
        self.display.gamma = s.gamma;
        self.display.vblanks = s.vblanks;
        self.display.swapped = s.swapped;
        self.display.shown_lost = s.shown_lost;
        self.display.pending = s.pending_vblanks;
        self.active_callback = None;
        self.undelivered = None;
        self.drop_visibility_tests();
        for (index, time, swap, count) in s.visibility {
            self.visibility.insert(
                index,
                VisibilityTest {
                    time,
                    swap,
                    result: VisibilityResult::Count(count),
                },
            );
        }
        self.pending_destroys = s.pending_destroys;
        self.torn_down = s.torn_down.into_iter().collect();
        Ok(())
    }

    /// Forgets every host copy of guest memory and every host-only surface
    /// that is not the swap ring or the depth buffer, without touching
    /// guest memory: texture, vertex and index caches, translation and draw
    /// memos, render-to-texture targets, the alias mirror, depth copies and
    /// aliases. Each refills from guest memory when next used. Pixels only
    /// on the host (targets not written back, surfaces of other headers)
    /// are lost; [`Device::save_snapshot`] writes the targets back first.
    pub fn drop_host_caches(&mut self) -> Result<(), D3dError> {
        self.vertex_memo = None;
        self.world_vertex_plans = Default::default();
        self.opaque_targets = None;
        self.translated = Default::default();
        self.stream_pool.clear();
        self.stream_pool_bytes = 0;
        self.stream_scratch.clear();
        if let Some(c) = &mut self.vertex_cache {
            c.clear();
        }
        if let Some(c) = &mut self.index_cache {
            c.clear();
        }
        self.push_words.clear();
        self.microcode_memo = None;
        self.microcode_seen.clear();
        self.constants_generation += 1;
        self.draw_constants = None;
        self.draw_pixel_shader = None;
        self.draw_combiner = None;
        self.cache.clear();
        self.mirror = None;
        self.mirror_stale.clear();
        self.rtt_synced.clear();
        self.rtt_stores.clear();
        self.depth_level_tex.clear();
        self.texture_flushed.clear();
        self.rtt_epoch += 1;
        let mut surfaces: Vec<B::Surface> = self.rtt.drain().map(|(_, t)| t.surface).collect();
        surfaces.extend(self.other_surfaces.drain().map(|(_, (_, s))| s));
        for s in surfaces {
            self.drop_surface(s)?;
        }
        self.depth_generation += 1;
        self.depth_copy = None;
        self.depth_alias_decodes.clear();
        self.depth_read_generation = None;
        self.depth_alias.clear();
        self.frame_snaps.clear();
        self.depth_color = None;
        self.depth_color_newer = false;
        Ok(())
    }
}
