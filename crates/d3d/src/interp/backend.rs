//! [`InterpBackend`]: records each frame's draws into the presented
//! surface and replays the newest one with interpolated matrices into
//! surfaces of its own, at the display rate, on the render thread.

use std::sync::{Arc, Mutex};

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use super::model::{FrameModel, Substituted, Tuning, match_draws, substitute};
use super::pacing::{Rate, Schedule, alpha, blendable, defer};
use crate::backend::{
    Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw, StreamBytes,
};

/// Counters, shared with whoever wants to print them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InterpStats {
    /// Guest frames presented while the replay was on.
    pub frames: u64,
    /// Of those, frames shown blended (a replay at the present).
    pub blended_frames: u64,
    /// Frames shown as the guest drew them: no previous frame to blend
    /// from, a gap, a view change, or nothing to move.
    pub held_frames: u64,
    /// In-between frames replayed between presents.
    pub replays: u64,
    /// Draws replayed with substituted constants, over all replays.
    pub substituted_draws: u64,
    /// Views or blocks snapped instead of blended (cuts, teleports).
    pub snapped: u64,
    /// Idle calls that put a replay off because a pixel count was open.
    pub deferred: u64,
}

impl std::fmt::Display for InterpStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "frames {} blended {} held {} replays {} substituted draws {} snapped {} deferred {}",
            self.frames,
            self.blended_frames,
            self.held_frames,
            self.replays,
            self.substituted_draws,
            self.snapped,
            self.deferred
        )
    }
}

enum Op<B: Backend> {
    Clear {
        color: B::Surface,
        depth: Option<B::Surface>,
        clear: Clear,
    },
    ClearRects {
        color: B::Surface,
        depth: Option<B::Surface>,
        clear: RectClear,
    },
    Draw {
        color: B::Surface,
        depth: Option<B::Surface>,
        draw: Box<ResolvedDraw>,
        textures: [Option<Arc<B::Texture>>; 4],
    },
    Copy {
        src: B::Surface,
        dst: B::Surface,
    },
}

impl<B: Backend> Op<B> {
    fn targets(&self, s: B::Surface) -> bool {
        match self {
            Op::Clear { color, .. } | Op::ClearRects { color, .. } | Op::Draw { color, .. } => {
                *color == s
            }
            Op::Copy { dst, .. } => *dst == s,
        }
    }
}

struct Frame<B: Backend> {
    id: u64,
    surface: B::Surface,
    /// Arrival (ns), stamped on the game's thread.
    at: u64,
    ops: Vec<Op<B>>,
    model: FrameModel,
    /// For each draw of `model`, the draw of the previous frame it
    /// continues.
    matches: Vec<Option<usize>>,
}

#[derive(Clone, Copy, Debug)]
struct SurfaceInfo {
    format: D3DFormat,
    width: u32,
    height: u32,
    depth: bool,
}

/// A [`Backend`] that passes everything to `B` and, with the frame-rate
/// setting above 30, shows in-between frames: each guest frame's draws into
/// the presented surface are kept, and the newest frame is drawn again into
/// a surface of its own with camera and object matrices blended from the
/// frame before (docs/game/high-framerate.md, "Phase 2").
///
/// Nothing the guest can observe goes through the replay: it draws only
/// into its own surfaces, never reads back, never runs while a pixel count
/// is open, and the guest's own calls reach `B` exactly as without it.
/// With the setting at 30 it records nothing and forwards every call.
pub struct InterpBackend<B: Backend> {
    inner: B,
    rate: Box<dyn Fn() -> u32 + Send>,
    clock: Box<dyn Fn() -> u64 + Send>,
    base: std::time::Instant,
    tuning: Tuning,
    surfaces: Vec<(B::Surface, SurfaceInfo)>,
    /// Surfaces presented so far (the swap chain).
    shown: Vec<B::Surface>,
    /// The frame being recorded.
    ops: Vec<Op<B>>,
    /// The arrival of the present about to run ([`Backend::stamp_present`]).
    stamp: Option<u64>,
    prev: Option<Frame<B>>,
    cur: Option<Frame<B>>,
    /// Each real surface's private copy.
    private: Vec<(B::Surface, B::Surface)>,
    /// A pixel count is open (cleared, not yet reported): no replay may
    /// add to it.
    counting: bool,
    schedule: Schedule,
    /// The frame and alpha last shown.
    last: Option<(u64, f32)>,
    gamma: Option<Box<[u8; 768]>>,
    next_id: u64,
    failed: bool,
    stats: Arc<Mutex<InterpStats>>,
}

impl<B: Backend> InterpBackend<B> {
    /// Wraps `inner`; `rate` reads the frame-rate setting (30 off, 0
    /// uncapped), on the render thread, at every present and idle call.
    pub fn new(inner: B, rate: impl Fn() -> u32 + Send + 'static) -> Self {
        let base = std::time::Instant::now();
        Self {
            inner,
            rate: Box::new(rate),
            clock: Box::new(move || base.elapsed().as_nanos() as u64),
            base,
            tuning: Tuning::default(),
            surfaces: Vec::new(),
            shown: Vec::new(),
            ops: Vec::new(),
            stamp: None,
            prev: None,
            cur: None,
            private: Vec::new(),
            counting: false,
            schedule: Schedule::default(),
            last: None,
            gamma: None,
            next_id: 1,
            failed: false,
            stats: Arc::default(),
        }
    }

    /// Uses `clock` (nanoseconds) instead of wall time: a fake display
    /// clock for tests. Stamps given to [`Backend::stamp_present`] must be
    /// on the same clock.
    pub fn with_clock(mut self, clock: impl Fn() -> u64 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// Nanoseconds on this backend's clock for `at` (for stamps made with
    /// the wall clock).
    pub fn wall_ns(base: std::time::Instant, at: std::time::Instant) -> u64 {
        at.saturating_duration_since(base).as_nanos() as u64
    }

    /// The counters.
    pub fn stats(&self) -> Arc<Mutex<InterpStats>> {
        Arc::clone(&self.stats)
    }

    /// The wrapped backend.
    pub fn inner(&self) -> &B {
        &self.inner
    }

    /// The wrapped backend.
    pub fn inner_mut(&mut self) -> &mut B {
        &mut self.inner
    }

    fn rate(&self) -> Rate {
        if self.failed {
            Rate::Off
        } else {
            Rate::from_setting((self.rate)())
        }
    }

    fn recording(&self, color: B::Surface) -> bool {
        self.shown.contains(&color) && self.rate() != Rate::Off
    }

    fn info(&self, s: B::Surface) -> Option<SurfaceInfo> {
        self.surfaces.iter().find(|(x, _)| *x == s).map(|(_, i)| *i)
    }

    /// Forgets the frames (their surfaces or textures are going away, or
    /// the replay is off).
    fn reset(&mut self) {
        self.prev = None;
        self.cur = None;
        self.ops.clear();
        self.last = None;
    }

    fn private_of(&mut self, s: B::Surface) -> Result<B::Surface, String> {
        if let Some((_, p)) = self.private.iter().find(|(r, _)| *r == s) {
            return Ok(*p);
        }
        let i = self.info(s).ok_or_else(|| {
            format!("interpolation: surface {s:?} was not made through this backend")
        })?;
        let p = self
            .inner
            .create_surface(i.format, i.width, i.height, i.depth)?;
        self.private.push((s, p));
        Ok(p)
    }

    /// Draws the newest frame at `alpha` into the private copy of its
    /// surface and returns that copy.
    fn replay(&mut self, alpha: f32) -> Result<(B::Surface, Substituted), String> {
        let (Some(prev), Some(cur)) = (self.prev.as_ref(), self.cur.as_ref()) else {
            return Err("interpolation: no frames to replay".into());
        };
        let draws: Vec<&ResolvedDraw> = cur
            .ops
            .iter()
            .filter_map(|o| match o {
                Op::Draw { draw, .. } => Some(&**draw),
                _ => None,
            })
            .collect();
        let (subs, st) = substitute(
            &prev.model,
            &cur.model,
            &cur.matches,
            alpha,
            &self.tuning,
            &|i| draws[i].shader.constants.clone(),
        );
        drop(draws);
        // Private surfaces for every target the frame touches.
        let mut targets: Vec<B::Surface> = vec![cur.surface];
        for o in &cur.ops {
            match o {
                Op::Clear { depth: Some(d), .. }
                | Op::ClearRects { depth: Some(d), .. }
                | Op::Draw { depth: Some(d), .. } => {
                    if !targets.contains(d) {
                        targets.push(*d);
                    }
                }
                _ => {}
            }
        }
        let mut map = Vec::with_capacity(targets.len());
        for t in targets {
            map.push((t, self.private_of(t)?));
        }
        let to = |s: B::Surface| map.iter().find(|(r, _)| *r == s).map_or(s, |(_, p)| *p);
        let cur = self.cur.as_mut().expect("checked above");
        let surface = to(cur.surface);
        let mut k = 0;
        for o in cur.ops.iter_mut() {
            match o {
                Op::Clear {
                    color,
                    depth,
                    clear,
                } => {
                    self.inner.clear(to(*color), depth.map(to), *clear)?;
                }
                Op::ClearRects {
                    color,
                    depth,
                    clear,
                } => {
                    self.inner.clear_rects(to(*color), depth.map(to), clear)?;
                }
                Op::Copy { src, dst } => {
                    self.inner.copy_surface(to(*src), to(*dst))?;
                }
                Op::Draw {
                    color,
                    depth,
                    draw,
                    textures,
                } => {
                    let tex: [Option<&B::Texture>; 4] =
                        std::array::from_fn(|i| textures[i].as_deref());
                    match subs[k].clone() {
                        Some(c) => {
                            let own = std::mem::replace(&mut draw.shader.constants, c);
                            let r = self.inner.draw(to(*color), depth.map(to), draw, tex);
                            draw.shader.constants = own;
                            r?;
                        }
                        None => {
                            self.inner.draw(to(*color), depth.map(to), draw, tex)?;
                        }
                    }
                    k += 1;
                }
            }
        }
        Ok((surface, st))
    }

    /// The guest's present: closes the frame being recorded and shows it,
    /// blended from the one before when it can.
    fn end_frame(
        &mut self,
        surface: B::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        let rate = self.rate();
        let now = (self.clock)();
        let at = self.stamp.take().unwrap_or(now);
        let Some(period) = rate.period_ns() else {
            if self.cur.is_some() || !self.ops.is_empty() {
                self.reset();
            }
            if !self.shown.contains(&surface) {
                self.shown.push(surface);
            }
            return self.inner.present_with_gamma(surface, flags, gamma);
        };
        self.gamma = gamma.map(|g| Box::new(*g));
        let ops: Vec<Op<B>> = std::mem::take(&mut self.ops)
            .into_iter()
            .filter(|o| o.targets(surface))
            .collect();
        if !self.shown.contains(&surface) {
            self.shown.push(surface);
        }
        let model = {
            let draws: Vec<&ResolvedDraw> = ops
                .iter()
                .filter_map(|o| match o {
                    Op::Draw { draw, .. } => Some(&**draw),
                    _ => None,
                })
                .collect();
            FrameModel::build(&draws)
        };
        // The pair to blend: only consecutive frames close together, with
        // the same views (a split-screen change, a load or a pause starts
        // over).
        let keep = self.cur.take().filter(|c| {
            blendable(c.at, at)
                && !model.cameras.is_empty()
                && c.model.cameras.len() == model.cameras.len()
                && c.model
                    .cameras
                    .iter()
                    .zip(&model.cameras)
                    .all(|(a, b)| a.view == b.view)
        });
        let matches = keep
            .as_ref()
            .map(|p| match_draws(&p.model, &model))
            .unwrap_or_default();
        let id = self.next_id;
        self.next_id += 1;
        self.prev = keep;
        self.cur = Some(Frame {
            id,
            surface,
            at,
            ops,
            model,
            matches,
        });
        let mut st = self.stats.lock().unwrap();
        st.frames += 1;
        drop(st);
        let blend = self.prev.is_some()
            && !self.counting
            && self.cur.as_ref().is_some_and(|c| c.model.has_matrices());
        if blend {
            let a = alpha(now, self.prev.as_ref().unwrap().at, at);
            match self.replay(a) {
                Ok((p, s)) => {
                    let mut st = self.stats.lock().unwrap();
                    st.blended_frames += 1;
                    st.substituted_draws += s.draws as u64;
                    st.snapped += s.snapped as u64;
                    drop(st);
                    self.last = Some((id, a));
                    self.schedule.shown_at(now, period);
                    return self.inner.present_with_gamma(p, flags, gamma);
                }
                Err(e) => self.fail(e),
            }
        }
        self.stats.lock().unwrap().held_frames += 1;
        self.last = Some((id, 1.0));
        self.inner.present_with_gamma(surface, flags, gamma)
    }

    /// Turns the replay off for the rest of the run: a replay error is a
    /// host bug, and the guest's frames go on without it.
    fn fail(&mut self, e: String) {
        eprintln!("d3d: interpolation off for the rest of the run: {e}");
        self.failed = true;
        self.reset();
    }

    /// The idle step at `now` (ns): an in-between frame if one is due.
    /// Returns when it wants to be called again (ns), if at all.
    pub fn idle_at(&mut self, now: u64) -> Option<u64> {
        let period = self.rate().period_ns()?;
        let (prev_at, cur_id, cur_at) = match (&self.prev, &self.cur) {
            (Some(p), Some(c)) => (p.at, c.id, c.at),
            _ => return None,
        };
        if self.last.is_some_and(|(id, a)| id == cur_id && a >= 1.0) {
            return None;
        }
        if self.counting {
            self.stats.lock().unwrap().deferred += 1;
            return Some(now + 1_000_000);
        }
        if !self.schedule.is_due(now) {
            return self.schedule.deadline();
        }
        if let Some(wait) = defer(now, prev_at, cur_at, period) {
            return Some(wait);
        }
        self.schedule.due(now, period);
        let a = alpha(now, prev_at, cur_at);
        match self.replay(a) {
            Ok((p, s)) => {
                let gamma = self.gamma.take();
                let r = self.inner.present_replay(p, gamma.as_deref());
                self.gamma = gamma;
                if let Err(e) = r {
                    self.fail(e);
                    return None;
                }
                let mut st = self.stats.lock().unwrap();
                st.replays += 1;
                st.substituted_draws += s.draws as u64;
                drop(st);
                self.last = Some((cur_id, a));
                if a >= 1.0 {
                    None
                } else {
                    self.schedule.deadline()
                }
            }
            Err(e) => {
                self.fail(e);
                None
            }
        }
    }
}

impl<B: Backend> Drop for InterpBackend<B> {
    fn drop(&mut self) {
        let s = *self.stats.lock().unwrap();
        if s.frames > 0 {
            eprintln!("d3d: interpolation: {s}");
        }
    }
}

impl<B: Backend> Backend for InterpBackend<B>
where
    B::Texture: Sync,
{
    type Surface = B::Surface;
    type Texture = Arc<B::Texture>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let s = self.inner.create_surface(format, width, height, depth)?;
        self.surfaces.push((
            s,
            SurfaceInfo {
                format,
                width,
                height,
                depth,
            },
        ));
        Ok(s)
    }

    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        let in_frames = [&self.prev, &self.cur].iter().any(|f| {
            f.as_ref().is_some_and(|f| {
                f.surface == surface
                    || f.ops.iter().any(|o| match o {
                        Op::Clear { depth, .. }
                        | Op::ClearRects { depth, .. }
                        | Op::Draw { depth, .. } => *depth == Some(surface),
                        Op::Copy { src, .. } => *src == surface,
                    })
            })
        });
        if in_frames {
            self.reset();
        }
        self.ops.retain(|o| match o {
            Op::Clear { depth, .. } | Op::ClearRects { depth, .. } | Op::Draw { depth, .. } => {
                *depth != Some(surface)
            }
            Op::Copy { src, .. } => *src != surface,
        });
        self.shown.retain(|s| *s != surface);
        self.surfaces.retain(|(s, _)| *s != surface);
        if let Some(i) = self.private.iter().position(|(r, _)| *r == surface) {
            let (_, p) = self.private.remove(i);
            self.inner.destroy_surface(p)?;
        }
        self.inner.destroy_surface(surface)
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        self.inner.upload_texture(texture).map(Arc::new)
    }

    fn bc_supported(&self) -> bool {
        self.inner.bc_supported()
    }

    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String> {
        if self.recording(color) {
            self.ops.push(Op::Clear {
                color,
                depth,
                clear,
            });
        }
        self.inner.clear(color, depth, clear)
    }

    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
        let r = self.inner.draw(color, depth, draw, tex)?;
        if r == DrawOutcome::Drawn && self.recording(color) {
            self.ops.push(Op::Draw {
                color,
                depth,
                draw: Box::new(draw.clone()),
                textures: std::array::from_fn(|i| textures[i].cloned()),
            });
        }
        Ok(r)
    }

    fn draw_owned(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if !self.recording(color) {
            let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
            return self.inner.draw_owned(color, depth, draw, tex);
        }
        let tex: [Option<&B::Texture>; 4] = std::array::from_fn(|i| textures[i].map(|t| &**t));
        let r = self.inner.draw(color, depth, &draw, tex);
        if r == Ok(DrawOutcome::Drawn) {
            self.ops.push(Op::Draw {
                color,
                depth,
                draw: Box::new(draw),
                textures: std::array::from_fn(|i| textures[i].cloned()),
            });
        }
        (r, Vec::new())
    }

    fn frame_stats(&mut self) {
        self.inner.frame_stats()
    }

    fn stamp_present(&mut self, at: std::time::Instant) {
        self.stamp = Some(Self::wall_ns(self.base, at));
    }

    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.end_frame(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.end_frame(surface, flags, gamma)
    }

    fn present_replay(
        &mut self,
        surface: Self::Surface,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.inner.present_replay(surface, gamma)
    }

    fn idle(&mut self) -> Option<std::time::Instant> {
        let now = (self.clock)();
        self.idle_at(now)
            .map(|ns| self.base + std::time::Duration::from_nanos(ns))
    }

    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        let r = self.inner.clear_rects(color, depth, clear)?;
        if r == DrawOutcome::Drawn && self.recording(color) {
            self.ops.push(Op::ClearRects {
                color,
                depth,
                clear: clear.clone(),
            });
        }
        Ok(r)
    }

    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.inner
            .read_surface(surface, format, width, height, layout)
    }

    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.inner
            .read_depth(surface, format, width, height, layout)
    }

    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        let r = self.inner.copy_surface(src, dst)?;
        if r && self.recording(dst) {
            self.ops.push(Op::Copy { src, dst });
        }
        Ok(r)
    }

    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.inner.write_surface(surface, desc, bytes)
    }

    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        Ok(self.inner.snapshot_surface(surface, opaque)?.map(Arc::new))
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.counting = true;
        self.inner.clear_pixel_count()
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.counting = false;
        self.inner.report_pixel_count()
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.inner.read_pixel_count(ticket)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.inner.try_pixel_count(ticket)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.inner.drop_pixel_count(ticket)
    }

    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.inner.pixel_count_unsubmitted(ticket)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        let s = self.inner.create_texture_target(format, width, height)?;
        self.surfaces.push((
            s,
            SurfaceInfo {
                format,
                width,
                height,
                depth: false,
            },
        ));
        Ok(s)
    }

    fn can_sample_surfaces(&self) -> bool {
        self.inner.can_sample_surfaces()
    }

    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        Ok(self.inner.surface_texture(surface)?.map(Arc::new))
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.inner.can_sample_depth_surfaces()
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        Ok(self.inner.depth_level_texture(surface)?.map(Arc::new))
    }

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.inner.judge()
    }

    fn defers_depth_aliases(&self) -> bool {
        self.inner.defers_depth_aliases()
    }

    fn depth_alias_texture(
        &mut self,
        depth: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        Ok(self
            .inner
            .depth_alias_texture(
                depth, format, width, height, pitch, generation, offset, desc, options,
            )?
            .map(Arc::new))
    }

    fn can_mirror(&self) -> bool {
        self.inner.can_mirror()
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.inner.mirror_reserve(len)
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.inner.mirror_write(offset, bytes)
    }

    fn mirror_store(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.inner
            .mirror_store(surface, format, width, height, layout, offset)
    }

    fn mirror_load(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.inner
            .mirror_load(surface, format, width, height, layout, offset)
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.inner.mirror_read(offset, len)
    }
}
