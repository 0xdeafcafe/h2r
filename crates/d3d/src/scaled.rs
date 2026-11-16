//! Rendering above the guest's resolution without the guest seeing it
//! (docs/high-resolution.md, phases 1 and 2).
//!
//! [`ScaledBackend`] holds two backends that get the same calls. The
//! *reference* renders at the guest's size and answers everything the guest
//! can observe: surface and depth read-backs, pixel counts, draw outcomes,
//! the judge and every capability question. The *view* renders the frame
//! buffers and the depth buffer at an integer multiple of their size and is
//! what is shown. Nothing the view does reaches the device, so the guest's
//! values are the 1x run's by construction.
//!
//! What the view keeps at scale: every surface made at the frame size (the
//! size of the first colour surface, the back buffer), with
//! [`Backend::create_surface`] or as a render-to-texture level
//! ([`Backend::create_texture_target`], an intermediate like Halo's
//! texaccum): the frame buffers, the depth buffer, copies of them and
//! back-buffer-sized targets. Their snapshots are copies of the scaled
//! image. Every other surface (smaller render-to-texture levels, other
//! sizes) lives only in the reference, and the view samples the
//! reference's texture of it (the two share a device), so offscreen passes
//! run once. Textures are uploaded once and shared the same way. A texture
//! over the whole depth buffer is made from the view's scaled depth when the
//! view can make it on the GPU; other depth textures are the reference's.
//!
//! The view draws everything the reference draws into a scaled target. What
//! it cannot draw the same way it repairs from the reference, per draw:
//!
//! - a draw into a guest-size target with the scaled depth buffer bound
//!   that writes depth or stencil is drawn again into the view's depth,
//!   through a scaled colour surface of the depth's size with colour writes
//!   off ([`ScaleStats::proxy_depth_draws`]);
//! - guest bytes written into a scaled surface, colour or depth, are copied
//!   from the reference's, each guest pixel repeated;
//! - a draw the view cannot make, or makes with another outcome (a texture
//!   it cannot sample, a depth surface that is not scaled), is replaced by
//!   the reference's target, each guest pixel repeated
//!   ([`ScaleStats::color_resyncs`], [`ScaleStats::depth_resyncs`]). That
//!   target is then at 1x until it is drawn again, but it has no holes.
//!
//! Each kind of repair is logged once. Stipple is still indexed by host
//! pixel.
//!
//! A view failure never reaches the guest: it is counted and that frame is
//! shown from the reference's image.
//!
//! **Lazy reference** ([`ScaledBackend::set_lazy_reference`],
//! `H2_LAZY_REFERENCE=1`): the reference's draws and whole clears into scaled surfaces
//! are queued until something needs those surfaces, and a whole clear
//! drops queued work nothing read, so a frame nobody reads back is drawn
//! once, at scale.
//!
//! **Changing the scale during a run.** [`ScaledBackend::follow_scale`]
//! gives it a source of the wanted scale (the Video screen's Resolution
//! setting), read at every present. A change is made at that frame
//! boundary, on whichever thread presents (the render thread under
//! [`crate::ThreadedBackend`]): every scaled surface is made again at the
//! new scale and filled from the reference's, and the device's handles are
//! mapped onto the new ones, so the guest and the device see nothing. At
//! scale 1 the view sleeps: it draws nothing and each frame is the
//! reference's ([`ScaleStats::reference_frames`]), so a run can start at
//! 1x and go up without a second backend's work until it does.

use std::sync::{Arc, Mutex};

use gfx::state::{ColorWriteMask, DepthBufferMode, StencilOp};
use gfx::texture::{DecodeOptions, DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{Backend, DrawJudge, DrawOutcome, GuestLayoutKind, RectClear, ResolvedDraw};
use crate::gfx_backend::{GfxBackend, HostTexture};

/// The largest render scale: 6x of 640x480 is 3840x2880.
pub const MAX_SCALE: u32 = 6;

/// `H2_RENDER_SCALE`: unset, empty or `1` is 1 (no view backend at all);
/// `2`..=`6` render the frame at that multiple.
pub fn render_scale_from(value: Option<&str>) -> Result<u32, String> {
    let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(1);
    };
    match v.parse::<u32>() {
        Ok(n) if (1..=MAX_SCALE).contains(&n) => Ok(n),
        _ => Err(format!(
            "H2_RENDER_SCALE={v:?}: expected a whole number from 1 to {MAX_SCALE}"
        )),
    }
}

/// [`render_scale_from`] of the environment.
pub fn render_scale() -> Result<u32, String> {
    render_scale_from(std::env::var("H2_RENDER_SCALE").ok().as_deref())
}

/// The view backend for `scale`, made only above 1: at 1 the run is
/// today's single backend, with no second one built.
pub fn view_for<V>(scale: u32, make: impl FnOnce() -> V) -> Option<V> {
    (scale > 1).then(make)
}

/// What a view backend needs beyond [`Backend`]: surfaces at a multiple of
/// their guest size, and access to its reference `R`'s resources (the two
/// are on one device).
pub trait ScaleView<R: Backend>: Backend {
    /// A surface `scale` times `width x height`. Draws and clears into it
    /// keep taking guest-pixel rectangles; the backend scales them.
    fn create_scaled_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
        scale: u32,
    ) -> Result<Self::Surface, String>;
    /// A render-to-texture colour target `scale` times `width x height`
    /// ([`Backend::create_texture_target`] at scale).
    fn create_scaled_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<Self::Surface, String>;
    /// Counted draws stop counting (the reference answers every count).
    fn stop_counting(&mut self);
    /// This backend's handle for `reference`'s texture. `None`: it cannot
    /// sample it.
    fn share_texture(&mut self, reference: &R, texture: &R::Texture) -> Option<Self::Texture>;
    /// Submits work recorded so far, before the reference changes a
    /// surface this backend may be sampling.
    fn submit_recorded(&mut self) -> Result<(), String>;
    /// Fills scaled surface `dst` from `reference`'s guest-size surface
    /// `src` of the same kind (colour or depth/stencil), each guest pixel
    /// repeated. `Ok(false)`: it cannot (the depth cannot be read back).
    fn fill_from(
        &mut self,
        reference: &mut R,
        src: R::Surface,
        dst: Self::Surface,
    ) -> Result<bool, String>;
    /// For the lazy reference ([`ScaledBackend::set_lazy_reference`]): a
    /// handle keeping `reference`'s `texture` alive until a deferred draw
    /// runs, and whether the reference's draws sample it through a view.
    /// `None` (the default): the reference's draws are never deferred.
    fn retain_reference_texture(&self, _texture: &R::Texture) -> Option<(R::Texture, bool)> {
        None
    }
    /// Whether `reference`'s present reads the surface it presents (a
    /// present hook): its deferred draws must run first.
    fn reference_present_reads(&self, _reference: &R) -> bool {
        true
    }
    /// Ends the frame showing `reference`'s `surface` instead of one of
    /// its own (the fallback after a view failure).
    fn present_reference(
        &mut self,
        reference: &R,
        surface: R::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String>;
}

/// A [`ScaledBackend`] surface: the reference's, and the view's when the
/// surface is scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScaledSurface<R, V> {
    /// The reference's surface.
    pub reference: R,
    /// The view's scaled surface; `None` for a surface only the reference
    /// keeps (or one the view failed to make).
    pub view: Option<V>,
    /// Guest size.
    pub size: (u32, u32),
}

/// A [`ScaledBackend`] texture.
#[derive(Debug)]
pub struct ScaledTexture<R, V, R2 = ()> {
    /// The reference's texture.
    pub reference: R,
    /// What the view samples: its own (a snapshot of a scaled surface) or
    /// the reference's, shared. `None`: the view cannot sample it.
    pub view: Option<V>,
    /// The reference surface it is a view of in place
    /// ([`Backend::surface_texture`]): what a draw sampling it reads depends
    /// on when the draw runs.
    pub(crate) live: Option<R2>,
}

/// What the view did differently from the reference. None of it reaches
/// the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScaleStats {
    /// View calls that failed.
    pub view_errors: u64,
    /// View draws and clears whose outcome differed from the reference's
    /// (each repaired from the reference: `color_resyncs`).
    pub mismatches: u64,
    /// Draws, clears and guest writes that changed the reference's depth
    /// buffer and could not be repeated in the view's (its depth cannot be
    /// read back to copy).
    pub depth_missed: u64,
    /// Frames shown from the reference after a view failure.
    pub fallback_frames: u64,
    /// Frames shown from the view.
    pub view_frames: u64,
    /// Scaled colour targets replaced by the reference's (a draw the view
    /// could not make the same way).
    pub color_resyncs: u64,
    /// Scaled depth surfaces replaced by the reference's (guest writes, or
    /// a depth-writing draw the view could not repeat).
    pub depth_resyncs: u64,
    /// Depth-writing draws into a guest-size target repeated into the
    /// view's scaled depth with colour writes off.
    pub proxy_depth_draws: u64,
    /// Textures over the depth buffer made from the view's scaled depth.
    pub scaled_depth_aliases: u64,
    /// Frames shown from the reference because the scale is 1 (the view
    /// sleeps): not failures.
    pub reference_frames: u64,
    /// Scale changes made during the run.
    pub rescales: u64,
    /// The scale in effect.
    pub scale: u32,
    /// Reference draws and clears into scaled surfaces deferred
    /// ([`ScaledBackend::set_lazy_reference`]).
    pub reference_deferred: u64,
    /// Of those, run later because something needed the reference.
    pub reference_replayed: u64,
    /// Of those, never run: a whole clear of their target came first and
    /// nothing read what they drew.
    pub reference_dropped: u64,
    /// Deferred draws whose outcome when run differed from the judge's
    /// answer given at the time (a judge bug; the guest saw the judge's).
    pub lazy_mismatches: u64,
}

impl std::fmt::Display for ScaleStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} view frames, {} fallback frames, {} view errors, {} mismatches, {} depth missed, \
             {} colour resyncs, {} depth resyncs, {} depth-only proxy draws, {} scaled depth aliases, \
             {} 1x frames, {} scale changes, reference work {} deferred, {} run late, {} dropped, \
             {} lazy mismatches",
            self.view_frames,
            self.fallback_frames,
            self.view_errors,
            self.mismatches,
            self.depth_missed,
            self.color_resyncs,
            self.depth_resyncs,
            self.proxy_depth_draws,
            self.scaled_depth_aliases,
            self.reference_frames,
            self.rescales,
            self.reference_deferred,
            self.reference_replayed,
            self.reference_dropped,
            self.lazy_mismatches
        )
    }
}

/// Repairs logged once each ([`ScaledBackend::note_once`]).
#[derive(Clone, Copy)]
enum Repair {
    Texture = 1,
    DepthNotScaled = 2,
    Outcome = 4,
    DepthGuestSize = 8,
    DepthWrite = 16,
    DepthUnreadable = 32,
}

/// A reference backend at the guest's size and a view backend at `scale`
/// (module docs).
pub struct ScaledBackend<R: Backend, V: ScaleView<R>> {
    reference: R,
    view: V,
    scale: u32,
    /// The frame size: the first colour surface's (the back buffer).
    frame: Option<(u32, u32)>,
    /// Every scaled surface, in the order made: the colour ones serve
    /// clears and depth-only draws of the scaled depth buffer bound with a
    /// guest-size colour target, and all are made again on a scale change.
    scaled: Vec<Scaled<R::Surface, V::Surface>>,
    /// A scale change has mapped a handle the device holds onto another.
    remapped: bool,
    /// Where the wanted scale comes from ([`ScaledBackend::follow_scale`]).
    follow: Option<Box<dyn Fn() -> u32 + Send>>,
    /// The view has recorded work not yet submitted.
    view_pending: bool,
    /// A view call failed since the last present.
    frame_failed: bool,
    stats: ScaleStats,
    /// [`Repair`]s already logged.
    logged: u32,
    /// A copy of `stats` updated at every present, for a reader on another
    /// thread ([`ScaledBackend::shared_stats`]).
    shared: Option<Arc<Mutex<ScaleStats>>>,
    /// The reference's judge while the lazy reference is on
    /// ([`ScaledBackend::set_lazy_reference`]).
    judge: Option<Box<dyn DrawJudge>>,
    /// Reference work into scaled surfaces not run yet, in order.
    deferred: Vec<Deferred<R>>,
}

/// Deferred work past this many calls runs (a bound on memory when no
/// whole clear comes).
const MAX_DEFERRED: usize = 16_384;

/// A reference call into scaled surfaces, deferred.
enum Deferred<R: Backend> {
    Clear {
        color: R::Surface,
        depth: Option<R::Surface>,
        clear: Clear,
    },
    Draw {
        color: R::Surface,
        depth: Option<R::Surface>,
        draw: Box<ResolvedDraw>,
        textures: [Option<R::Texture>; 4],
        /// What the device was told.
        judged: DrawOutcome,
        /// The surfaces it samples in place.
        live: Vec<R::Surface>,
    },
}

impl<R: Backend> Deferred<R> {
    fn surfaces(&self) -> (R::Surface, Option<R::Surface>) {
        match self {
            Deferred::Clear { color, depth, .. } | Deferred::Draw { color, depth, .. } => {
                (*color, *depth)
            }
        }
    }

    /// The depth surface it may change.
    fn writes_depth(&self) -> Option<R::Surface> {
        match self {
            Deferred::Clear { depth, clear, .. } => {
                depth.filter(|_| clear.depth.is_some() || clear.stencil.is_some())
            }
            Deferred::Draw { depth, draw, .. } => depth.filter(|_| writes_depth_or_stencil(draw)),
        }
    }

    fn live(&self) -> &[R::Surface] {
        match self {
            Deferred::Draw { live, .. } => live,
            Deferred::Clear { .. } => &[],
        }
    }

    /// The surfaces it overwrites whole: a clear's colour, and its depth
    /// surface when it clears depth and stencil.
    fn overwrites(&self) -> [Option<R::Surface>; 2] {
        match self {
            Deferred::Clear {
                color,
                depth,
                clear,
            } => [
                clear.color.is_some().then_some(*color),
                depth.filter(|_| clear.depth.is_some() && clear.stencil.is_some()),
            ],
            Deferred::Draw { .. } => [None, None],
        }
    }

    /// The surfaces whose earlier contents its result depends on. A clear
    /// overwrites what it clears (a depth clear that keeps the stencil, or
    /// the reverse, depends on the surface); a draw depends on its colour
    /// target, and on its depth surface when it tests or writes it.
    fn reads(&self) -> [Option<R::Surface>; 2] {
        match self {
            Deferred::Clear { depth, clear, .. } => [
                None,
                depth.filter(|_| clear.depth.is_some() != clear.stencil.is_some()),
            ],
            Deferred::Draw {
                color, depth, draw, ..
            } => {
                let r = &draw.state.render;
                let uses_depth = r.depth_mode != DepthBufferMode::Disabled || r.stencil.enable;
                [Some(*color), depth.filter(|_| uses_depth)]
            }
        }
    }
}

/// `H2_LAZY_REFERENCE`: `1`, `on` or `true` turn the lazy reference on;
/// it is off otherwise (see [`ScaledBackend::set_lazy_reference`] for why).
pub fn lazy_reference_from(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "on" | "true"))
}

/// A scaled surface of the view: the handle the device holds (`original`),
/// the one in use now, and what it takes to make it again.
struct Scaled<R, V> {
    original: V,
    current: V,
    reference: R,
    size: (u32, u32),
    format: D3DFormat,
    depth: bool,
    /// Made by [`Backend::create_texture_target`].
    target: bool,
}

/// Whether `draw` can change the depth or stencil buffer it is bound with.
fn writes_depth_or_stencil(draw: &ResolvedDraw) -> bool {
    let r = &draw.state.render;
    let z = r.depth_mode != DepthBufferMode::Disabled && r.depth_write;
    let s = &r.stencil;
    let stencil = s.enable
        && s.write_mask != 0
        && [s.fail, s.depth_fail, s.pass]
            .iter()
            .any(|op| *op != StencilOp::Keep);
    z || stencil
}

impl<R: Backend, V: ScaleView<R>> ScaledBackend<R, V> {
    /// `reference` answers the guest; `view` renders at `scale` (1 to
    /// [`MAX_SCALE`]) and presents. They must share a device.
    pub fn new(reference: R, mut view: V, scale: u32) -> Result<Self, String> {
        if !(1..=MAX_SCALE).contains(&scale) {
            return Err(format!("render scale {scale} is not 1 to {MAX_SCALE}"));
        }
        view.stop_counting();
        Ok(Self {
            reference,
            view,
            scale,
            frame: None,
            scaled: Vec::new(),
            remapped: false,
            follow: None,
            view_pending: false,
            frame_failed: false,
            stats: ScaleStats {
                scale,
                ..ScaleStats::default()
            },
            logged: 0,
            shared: None,
            judge: None,
            deferred: Vec::new(),
        })
        .map(|mut b| {
            b.set_lazy_reference(lazy_reference_from(
                std::env::var("H2_LAZY_REFERENCE").ok().as_deref(),
            ));
            b
        })
    }

    /// The lazy reference (off by default; `H2_LAZY_REFERENCE=1` turns it
    /// on: it saves render-thread time only when frames are not read back,
    /// and costs a little per draw when they are; `scale_bench`). The reference's draws and whole clears into scaled surfaces
    /// (the frame buffers and frame-sized targets, which the view draws
    /// itself) are queued instead of run. Every other reference call runs
    /// the queue first, in order, so whatever the guest reads, or the
    /// reference draws from them, is what running each at once gives. A
    /// whole clear of a target drops the queued work that only drew into
    /// it (when nothing queued reads it): a frame whose back buffer and
    /// depth nobody reads is never drawn at 1x. Counted draws are never
    /// queued. A queued draw's outcome is the reference's judge's
    /// ([`Backend::judge`], what the render thread answers the guest with
    /// anyway). Off without a judge, or when the view cannot keep the
    /// reference's textures alive.
    pub fn set_lazy_reference(&mut self, on: bool) {
        if !on {
            if let Err(e) = self.settle() {
                eprintln!("d3d: render scale: deferred reference work: {e}");
            }
            self.judge = None;
        } else if self.judge.is_none() {
            self.judge = self.reference.judge();
        }
    }

    /// Runs the deferred reference work, in order.
    fn settle(&mut self) -> Result<(), String> {
        if self.deferred.is_empty() {
            return Ok(());
        }
        for op in std::mem::take(&mut self.deferred) {
            self.stats.reference_replayed += 1;
            match op {
                Deferred::Clear {
                    color,
                    depth,
                    clear,
                } => self.reference.clear(color, depth, clear)?,
                Deferred::Draw {
                    color,
                    depth,
                    draw,
                    textures,
                    judged,
                    ..
                } => {
                    let out = self.reference.draw(
                        color,
                        depth,
                        &draw,
                        std::array::from_fn(|i| textures[i].as_ref()),
                    )?;
                    if out != judged {
                        self.stats.lazy_mismatches += 1;
                        if self.stats.lazy_mismatches <= 10 {
                            eprintln!(
                                "d3d: render scale {}: deferred reference draw {out:?}, judged {judged:?}",
                                self.scale
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Runs the deferred work before reference work into `color` and
    /// `depth`, sampling surfaces `live` in place, that is not deferred, if
    /// the two depend on each other: it draws into or with a scaled
    /// surface, or samples one in place, or changes a surface deferred
    /// work samples in place. Work into guest-size targets whose textures
    /// are copies (snapshots, depth textures, uploads) touches nothing the
    /// deferred work does, so it may run first.
    fn settle_for(
        &mut self,
        color: &Surf<R, V>,
        depth: Option<&Surf<R, V>>,
        live: &[R::Surface],
    ) -> Result<(), String> {
        if self.deferred.is_empty() {
            return Ok(());
        }
        let scaled = |s: &R::Surface| self.scaled.iter().any(|x| x.reference == *s);
        let written = [Some(color.reference), depth.map(|d| d.reference)];
        let depends = color.view.is_some()
            || depth.is_some_and(|d| d.view.is_some())
            || live.iter().any(scaled)
            || self
                .deferred
                .iter()
                .any(|op| op.live().iter().any(|s| written.contains(&Some(*s))));
        if depends {
            self.settle()?;
        }
        Ok(())
    }

    /// Whether reference work into `color` and `depth` may be deferred.
    fn deferrable(&self, color: &Surf<R, V>, depth: Option<&Surf<R, V>>) -> bool {
        self.judge.is_some()
            && self.live(color.view).is_some()
            && depth.is_none_or(|d| self.live(d.view).is_some())
    }

    /// Queues `op`, running the queue when it is long.
    fn defer(&mut self, op: Deferred<R>) -> Result<(), String> {
        self.stats.reference_deferred += 1;
        self.deferred.push(op);
        if self.deferred.len() > MAX_DEFERRED {
            self.settle()?;
        }
        Ok(())
    }

    /// A whole clear of `color` (and `depth`) is about to be queued: drops
    /// the queued work whose every effect it overwrites and that no work
    /// kept after it reads.
    fn drop_covered(&mut self, color: R::Surface, depth: Option<R::Surface>, clear: Clear) {
        // A draw sampling a surface in place reads whatever is there when
        // it runs: keep everything.
        if self.deferred.iter().any(|op| !op.live().is_empty()) {
            return;
        }
        let colour = clear.color.is_some();
        let whole_depth = clear.depth.is_some() && clear.stencil.is_some();
        let covered = |op: &Deferred<R>| {
            let (c, _) = op.surfaces();
            c == color
                && colour
                && op
                    .writes_depth()
                    .is_none_or(|d| Some(d) == depth && whole_depth)
        };
        // From the end: a covered op goes unless a kept op after it depends
        // on what it writes.
        let mut needed: Vec<R::Surface> = Vec::new();
        let mut keep = vec![true; self.deferred.len()];
        for (i, op) in self.deferred.iter().enumerate().rev() {
            let (c, _) = op.surfaces();
            let writes = [Some(c), op.writes_depth()];
            if covered(op) && !writes.iter().flatten().any(|s| needed.contains(s)) {
                keep[i] = false;
                continue;
            }
            // What a kept op overwrites whole, the work before it cannot
            // be needed for (by the ops after it).
            needed.retain(|s| !op.overwrites().contains(&Some(*s)));
            for s in op.reads().into_iter().flatten() {
                if !needed.contains(&s) {
                    needed.push(s);
                }
            }
        }
        let before = self.deferred.len();
        let mut k = keep.into_iter();
        self.deferred.retain(|_| k.next().unwrap_or(true));
        self.stats.reference_dropped += (before - self.deferred.len()) as u64;
    }

    /// The scale in effect.
    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// Reads the wanted scale from `wanted` at every present and changes to
    /// it there (module docs). Values outside 1 to [`MAX_SCALE`] are
    /// clamped.
    pub fn follow_scale(&mut self, wanted: impl Fn() -> u32 + Send + 'static) {
        self.follow = Some(Box::new(wanted));
    }

    /// The view surface standing for `s` now: `None` while the view sleeps
    /// (scale 1) or for a surface only the reference keeps.
    pub fn view_surface(&self, s: &ScaledSurface<R::Surface, V::Surface>) -> Option<V::Surface> {
        self.live(s.view)
    }

    fn live(&self, v: Option<V::Surface>) -> Option<V::Surface> {
        if self.scale == 1 {
            return None;
        }
        let v = v?;
        if !self.remapped {
            return Some(v);
        }
        Some(
            self.scaled
                .iter()
                .find(|s| s.original == v)
                .map_or(v, |s| s.current),
        )
    }

    /// Makes every scaled surface again at `scale` and fills it from the
    /// reference's. Nothing changes if one cannot be made.
    fn rescale(&mut self, scale: u32) {
        let scale = scale.clamp(1, MAX_SCALE);
        if scale == self.scale {
            return;
        }
        let old = self.scale;
        self.settle_quietly();
        if let Err(e) = self.view.submit_recorded() {
            self.view_failed("submit", e);
        }
        self.view_pending = false;
        let mut made = Vec::with_capacity(self.scaled.len());
        for s in &self.scaled {
            let (w, h) = s.size;
            let r = if s.target {
                self.view
                    .create_scaled_texture_target(s.format, w, h, scale)
            } else {
                self.view
                    .create_scaled_surface(s.format, w, h, s.depth, scale)
            };
            match r {
                Ok(v) => made.push(v),
                Err(e) => {
                    for v in made {
                        let _ = self.view.destroy_surface(v);
                    }
                    eprintln!("d3d: render scale stays {old}: a {w}x{h} surface at {scale}x: {e}");
                    return;
                }
            }
        }
        self.scale = scale;
        self.stats.scale = scale;
        self.stats.rescales += 1;
        self.remapped = true;
        for (i, v) in made.into_iter().enumerate() {
            let old = std::mem::replace(&mut self.scaled[i].current, v);
            if let Err(e) = self.view.destroy_surface(old) {
                self.view_failed("destroy", e);
            }
        }
        // Waking or changing: the new surfaces start as the reference's
        // image, each guest pixel repeated, until they are drawn again.
        if scale > 1 {
            for i in 0..self.scaled.len() {
                let (r, v) = (self.scaled[i].reference, self.scaled[i].current);
                match self.view.fill_from(&mut self.reference, r, v) {
                    Ok(true) => {}
                    Ok(false) => self.stats.depth_missed += 1,
                    Err(e) => self.view_failed("fill", e),
                }
            }
        }
        eprintln!("d3d: render scale {old}x -> {scale}x");
    }

    /// What the view did differently so far.
    pub fn stats(&self) -> ScaleStats {
        self.stats
    }

    /// A handle to [`ScaledBackend::stats`] as of the last present, for a
    /// run report read on another thread (the backend may live on a render
    /// thread).
    pub fn shared_stats(&mut self) -> Arc<Mutex<ScaleStats>> {
        Arc::clone(
            self.shared
                .get_or_insert_with(|| Arc::new(Mutex::new(self.stats))),
        )
    }

    /// The reference. Its scaled surfaces may lag the guest's calls while
    /// work is deferred ([`ScaledBackend::set_lazy_reference`]);
    /// [`ScaledBackend::reference_mut`] runs it first.
    pub fn reference(&self) -> &R {
        &self.reference
    }

    /// The reference, mutably, with its deferred work run (so its
    /// surfaces hold what the guest's calls drew).
    pub fn reference_mut(&mut self) -> &mut R {
        self.settle_quietly();
        &mut self.reference
    }

    /// The view.
    pub fn view(&self) -> &V {
        &self.view
    }

    /// The view, mutably.
    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }

    fn view_failed(&mut self, what: &str, e: String) {
        self.stats.view_errors += 1;
        self.frame_failed = true;
        if self.stats.view_errors <= 10 {
            eprintln!(
                "d3d: render scale {}: view {what}: {e} (the frame is shown at 1x)",
                self.scale
            );
        }
    }

    /// Logs the first repair of kind `r`.
    fn note_once(&mut self, r: Repair, what: impl FnOnce() -> String) {
        if self.logged & r as u32 == 0 {
            self.logged |= r as u32;
            eprintln!(
                "d3d: render scale {}: {} (repaired from the 1x reference; logged once)",
                self.scale,
                what()
            );
        }
    }

    /// Compares the view's outcome with the reference's. `true`: the view
    /// did something else, and its target needs the reference's pixels.
    fn compare(
        &mut self,
        what: &str,
        reference: &DrawOutcome,
        view: Result<DrawOutcome, String>,
    ) -> bool {
        match view {
            Ok(v) if v == *reference => false,
            Ok(v) => {
                self.stats.mismatches += 1;
                self.note_once(Repair::Outcome, || {
                    format!("view {what} {v:?}, reference {reference:?}")
                });
                true
            }
            Err(e) => {
                self.view_failed(what, e);
                false
            }
        }
    }

    /// Before the reference changes guest-size surface `s`: the view's
    /// recorded draws that sample it must reach the GPU first.
    fn before_shared_write(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) {
        if self.live(s.view).is_none() && self.view_pending {
            self.view_pending = false;
            if let Err(e) = self.view.submit_recorded() {
                self.view_failed("submit", e);
            }
        }
    }

    /// Queues the reference's `draw` if it may be deferred; returns the
    /// judge's outcome for it then.
    fn defer_draw(
        &mut self,
        color: &Surf<R, V>,
        depth: Option<&Surf<R, V>>,
        draw: &ResolvedDraw,
        textures: [Option<&Tex<R, V>>; 4],
    ) -> Result<Option<DrawOutcome>, String> {
        if draw.hw.zpass_count_enable || !self.deferrable(color, depth) {
            return Ok(None);
        }
        let mut kept: [Option<R::Texture>; 4] = std::array::from_fn(|_| None);
        let mut views = [None; 4];
        let mut live = Vec::new();
        for (i, t) in textures.iter().enumerate() {
            let Some(t) = t else { continue };
            let Some((k, v)) = self.view.retain_reference_texture(&t.reference) else {
                return Ok(None);
            };
            kept[i] = Some(k);
            views[i] = Some(v);
            live.extend(t.live);
        }
        let judged = match self.judge.as_mut() {
            Some(j) => j.draw(draw, views),
            None => return Ok(None),
        };
        self.defer(Deferred::Draw {
            color: color.reference,
            depth: depth.map(|d| d.reference),
            draw: Box::new(draw.clone()),
            textures: kept,
            judged: judged.clone(),
            live,
        })?;
        Ok(Some(judged))
    }

    /// Runs the deferred reference work where the caller cannot return an
    /// error (a view repair): a failure is logged.
    fn settle_quietly(&mut self) {
        if let Err(e) = self.settle() {
            eprintln!(
                "d3d: render scale {}: deferred reference work: {e}",
                self.scale
            );
        }
    }

    /// A scaled colour surface of guest size `size`, and its format, to
    /// clear or draw the scaled depth through.
    fn proxy_color(&self, size: (u32, u32)) -> Option<(V::Surface, D3DFormat)> {
        self.scaled
            .iter()
            .find(|s| !s.depth && s.size == size)
            .map(|s| (s.current, s.format))
    }

    fn share(&mut self, t: &R::Texture) -> Option<V::Texture> {
        self.view.share_texture(&self.reference, t)
    }

    /// Refreshes the view's copy of scaled surface `s` from the
    /// reference's. `false`: it could not (counted).
    fn refill(&mut self, s: &ScaledSurface<R::Surface, V::Surface>) -> bool {
        let Some(v) = self.live(s.view) else {
            return true;
        };
        // The view's recorded draws into `v` come first; the fill replaces
        // them. The reference's deferred work comes first too.
        self.settle_quietly();
        self.view_pending = false;
        match self.view.fill_from(&mut self.reference, s.reference, v) {
            Ok(true) => true,
            Ok(false) => {
                self.stats.depth_missed += 1;
                self.note_once(Repair::DepthUnreadable, || {
                    "the reference's depth cannot be read back, so the view's misses a change"
                        .into()
                });
                false
            }
            Err(e) => {
                self.view_failed("fill", e);
                false
            }
        }
    }

    /// The view's `color` (and `depth`, when `draw` may write it) are
    /// replaced by the reference's after a draw the view could not repeat.
    fn resync(
        &mut self,
        color: &Surf<R, V>,
        depth: Option<&Surf<R, V>>,
        draw: Option<&ResolvedDraw>,
    ) {
        if self.live(color.view).is_some() && self.refill(color) {
            self.stats.color_resyncs += 1;
        }
        if let Some(d) = depth
            && self.live(d.view).is_some()
            && draw.is_none_or(writes_depth_or_stencil)
            && self.refill(d)
        {
            self.stats.depth_resyncs += 1;
        }
    }

    /// A draw into guest-size target `color` with the scaled depth `depth`
    /// bound wrote depth or stencil in the reference: the view repeats it
    /// into its depth through a scaled colour surface of the depth's size,
    /// colour writes off. Without one (or a texture it cannot sample) the
    /// view's depth is replaced by the reference's.
    fn depth_only_draw(
        &mut self,
        depth: &Surf<R, V>,
        draw: &ResolvedDraw,
        textures: [Option<&V::Texture>; 4],
        sampled: bool,
    ) {
        let Some(vd) = self.live(depth.view) else {
            return;
        };
        let proxy = self.proxy_color(depth.size).filter(|_| sampled);
        if let Some((proxy, format)) = proxy {
            let mut d = draw.clone();
            d.state.render.color_write = ColorWriteMask {
                red: false,
                green: false,
                blue: false,
                alpha: false,
            };
            // The proxy is the depth's size: window coordinates map onto
            // it as they do onto the depth buffer.
            (d.target.width, d.target.height) = depth.size;
            d.target.format = format;
            let r = self.view.draw(proxy, Some(vd), &d, textures);
            self.view_pending = true;
            match r {
                Ok(DrawOutcome::Drawn) => {
                    self.stats.proxy_depth_draws += 1;
                    return;
                }
                Ok(o) => self.note_once(Repair::DepthWrite, || {
                    format!("the view's depth-only draw was {o:?}")
                }),
                Err(e) => return self.view_failed("depth-only draw", e),
            }
        } else {
            self.note_once(Repair::DepthGuestSize, || {
                format!(
                    "a draw into a {}x{} target writes the scaled depth buffer and the view cannot repeat it",
                    draw.target.width, draw.target.height
                )
            });
        }
        if self.refill(depth) {
            self.stats.depth_resyncs += 1;
        }
    }
}

type Surf<R, V> = ScaledSurface<<R as Backend>::Surface, <V as Backend>::Surface>;
type Tex<R, V> =
    ScaledTexture<<R as Backend>::Texture, <V as Backend>::Texture, <R as Backend>::Surface>;

impl<R: Backend, V: ScaleView<R>> Backend for ScaledBackend<R, V> {
    type Surface = Surf<R, V>;
    type Texture = Tex<R, V>;

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<Self::Surface, String> {
        let reference = self
            .reference
            .create_surface(format, width, height, depth)?;
        if !depth && self.frame.is_none() {
            self.frame = Some((width, height));
        }
        let view = if self.frame == Some((width, height)) {
            match self
                .view
                .create_scaled_surface(format, width, height, depth, self.scale)
            {
                Ok(v) => Some(v),
                Err(e) => {
                    self.view_failed("surface", e);
                    None
                }
            }
        } else {
            None
        };
        if let Some(v) = view {
            self.scaled.push(Scaled {
                original: v,
                current: v,
                reference,
                size: (width, height),
                format,
                depth,
                target: false,
            });
        }
        Ok(ScaledSurface {
            reference,
            view,
            size: (width, height),
        })
    }

    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        self.settle()?;
        self.before_shared_write(&surface);
        self.reference.destroy_surface(surface.reference)?;
        if let Some(v) = surface.view {
            let at = self.scaled.iter().position(|s| s.original == v);
            let v = at.map_or(v, |i| self.scaled.remove(i).current);
            if let Err(e) = self.view.destroy_surface(v) {
                self.view_failed("destroy", e);
            }
        }
        Ok(())
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        let reference = self.reference.upload_texture(texture)?;
        let view = self.share(&reference);
        Ok(ScaledTexture {
            reference,
            view,
            live: None,
        })
    }

    fn bc_supported(&self) -> bool {
        self.reference.bc_supported()
    }

    fn clear(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: Clear,
    ) -> Result<(), String> {
        self.before_shared_write(&color);
        let (rc, rd) = (color.reference, depth.map(|d| d.reference));
        if self.deferrable(&color, depth.as_ref()) {
            self.drop_covered(rc, rd, clear);
            self.defer(Deferred::Clear {
                color: rc,
                depth: rd,
                clear,
            })?;
        } else {
            self.settle_for(&color, depth.as_ref(), &[])?;
            self.reference.clear(rc, rd, clear)?;
        }
        let vd = self.live(depth.and_then(|d| d.view));
        let r = match (self.live(color.view), depth) {
            (Some(vc), None) => self.view.clear(vc, None, clear),
            (Some(vc), Some(_)) if vd.is_some() => self.view.clear(vc, vd, clear),
            // The depth surface is not scaled: the view's colour only.
            (Some(vc), Some(_)) => self.view.clear(
                vc,
                None,
                Clear {
                    depth: None,
                    stencil: None,
                    ..clear
                },
            ),
            // A guest-size colour target with the scaled depth buffer:
            // clear the view's depth through a scaled colour of its size.
            (None, Some(d))
                if vd.is_some() && (clear.depth.is_some() || clear.stencil.is_some()) =>
            {
                match self.proxy_color(d.size) {
                    Some((proxy, _)) => self.view.clear(
                        proxy,
                        vd,
                        Clear {
                            color: None,
                            ..clear
                        },
                    ),
                    None => {
                        if self.refill(&d) {
                            self.stats.depth_resyncs += 1;
                        }
                        return Ok(());
                    }
                }
            }
            _ => return Ok(()),
        };
        self.view_pending = true;
        if let Err(e) = r {
            self.view_failed("clear", e);
        }
        Ok(())
    }

    fn draw(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        draw: &ResolvedDraw,
        textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        self.before_shared_write(&color);
        let out = match self.defer_draw(&color, depth.as_ref(), draw, textures)? {
            Some(judged) => judged,
            None => {
                let live: Vec<_> = textures.iter().flatten().filter_map(|t| t.live).collect();
                self.settle_for(&color, depth.as_ref(), &live)?;
                self.reference.draw(
                    color.reference,
                    depth.map(|d| d.reference),
                    draw,
                    textures.map(|t| t.map(|t| &t.reference)),
                )?
            }
        };
        let drawn = out == DrawOutcome::Drawn;
        let vd = self.live(depth.and_then(|d| d.view));
        let vt = textures.map(|t| t.and_then(|t| t.view.as_ref()));
        let sampled = textures
            .iter()
            .zip(&vt)
            .all(|(t, v)| t.is_none() || v.is_some());
        match (self.live(color.view), depth) {
            (Some(vc), d) if d.is_none() || vd.is_some() => {
                if !sampled {
                    // A texture the view cannot sample: it would read a
                    // stand-in. The reference's pixels instead.
                    if drawn {
                        self.note_once(Repair::Texture, || {
                            "a draw samples a texture the view cannot".into()
                        });
                        self.resync(&color, depth.as_ref(), Some(draw));
                    }
                    return Ok(out);
                }
                let r = self.view.draw(vc, vd, draw, vt);
                self.view_pending = true;
                if self.compare("draw", &out, r) {
                    self.resync(&color, depth.as_ref(), Some(draw));
                }
            }
            // Colour scaled, depth not: attachments of different sizes.
            (Some(_), Some(d)) => {
                if drawn {
                    self.note_once(Repair::DepthNotScaled, || {
                        format!(
                            "a draw into the scaled {}x{} target with a {}x{} depth surface",
                            color.size.0, color.size.1, d.size.0, d.size.1
                        )
                    });
                    self.resync(&color, None, None);
                }
            }
            (None, Some(d)) if vd.is_some() && drawn && writes_depth_or_stencil(draw) => {
                self.depth_only_draw(&d, draw, vt, sampled);
            }
            _ => {}
        }
        Ok(out)
    }

    fn frame_stats(&mut self) {
        self.reference.frame_stats();
        self.view.frame_stats();
    }

    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: Self::Surface,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        // The reference ends its frame too (its swap ring, shader upkeep);
        // it shows nothing, unless it has a present hook.
        if self.view.reference_present_reads(&self.reference) {
            self.settle()?;
        }
        self.reference
            .present_with_gamma(surface.reference, flags, gamma)?;
        self.view_pending = false;
        let shown = match self.live(surface.view) {
            Some(v) if !self.frame_failed => match self.view.present_with_gamma(v, flags, gamma) {
                Ok(()) => true,
                Err(e) => {
                    self.view_failed("present", e);
                    false
                }
            },
            _ => false,
        };
        if shown {
            self.stats.view_frames += 1;
        } else {
            if self.scale == 1 {
                self.stats.reference_frames += 1;
            } else {
                self.stats.fallback_frames += 1;
            }
            self.settle()?;
            if let Err(e) =
                self.view
                    .present_reference(&self.reference, surface.reference, flags, gamma)
            {
                self.stats.view_errors += 1;
                if self.stats.view_errors <= 10 {
                    eprintln!(
                        "d3d: render scale {}: showing the 1x frame: {e}",
                        self.scale
                    );
                }
            }
        }
        self.frame_failed = false;
        // The frame boundary: a wanted scale is made here, before the next
        // frame's first call.
        if let Some(wanted) = self.follow.as_ref().map(|f| f()) {
            self.rescale(wanted);
        }
        if let Some(s) = &self.shared
            && let Ok(mut s) = s.lock()
        {
            *s = self.stats;
        }
        Ok(())
    }

    fn clear_rects(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.settle_for(&color, depth.as_ref(), &[])?;
        self.before_shared_write(&color);
        let out = self
            .reference
            .clear_rects(color.reference, depth.map(|d| d.reference), clear)?;
        if out != DrawOutcome::Drawn {
            return Ok(out);
        }
        let vd = self.live(depth.and_then(|d| d.view));
        let touches_depth = clear.depth.is_some() || clear.stencil.is_some();
        let r = match (self.live(color.view), depth) {
            (Some(vc), d) if d.is_none() || vd.is_some() => self.view.clear_rects(vc, vd, clear),
            (Some(vc), Some(_)) => {
                let colour_only = RectClear {
                    depth: None,
                    stencil: None,
                    ..clear.clone()
                };
                self.view.clear_rects(vc, None, &colour_only)
            }
            (None, Some(d)) if vd.is_some() && touches_depth => match self.proxy_color(d.size) {
                Some((proxy, _)) => {
                    let depth_only = RectClear {
                        color: None,
                        ..clear.clone()
                    };
                    self.view.clear_rects(proxy, vd, &depth_only)
                }
                None => {
                    if self.refill(&d) {
                        self.stats.depth_resyncs += 1;
                    }
                    return Ok(out);
                }
            },
            _ => return Ok(out),
        };
        self.view_pending = true;
        if self.compare("clear", &out, r) {
            let depth = depth.filter(|_| touches_depth);
            self.resync(&color, depth.as_ref(), None);
        }
        Ok(out)
    }

    fn read_surface(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.settle()?;
        self.reference
            .read_surface(surface.reference, format, width, height, layout)
    }

    fn read_depth(
        &mut self,
        surface: Self::Surface,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.settle()?;
        self.reference
            .read_depth(surface.reference, format, width, height, layout)
    }

    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        self.settle()?;
        self.before_shared_write(&dst);
        let copied = self.reference.copy_surface(src.reference, dst.reference)?;
        if copied && let Some(vd) = self.live(dst.view) {
            match self.live(src.view) {
                Some(vs) => match self.view.copy_surface(vs, vd) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.refill(&dst);
                    }
                    Err(e) => self.view_failed("copy", e),
                },
                None => {
                    self.refill(&dst);
                }
            }
        }
        Ok(copied)
    }

    fn write_surface(
        &mut self,
        surface: Self::Surface,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.settle()?;
        self.before_shared_write(&surface);
        let written = self
            .reference
            .write_surface(surface.reference, desc, bytes)?;
        if written {
            self.refill(&surface);
        }
        Ok(written)
    }

    fn snapshot_surface(
        &mut self,
        surface: Self::Surface,
        opaque: bool,
    ) -> Result<Option<Self::Texture>, String> {
        self.settle()?;
        let Some(reference) = self.reference.snapshot_surface(surface.reference, opaque)? else {
            return Ok(None);
        };
        let view = match self.live(surface.view) {
            // The copy of the scaled image.
            Some(v) => match self.view.snapshot_surface(v, opaque) {
                Ok(t) => t,
                Err(e) => {
                    self.view_failed("snapshot", e);
                    None
                }
            },
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture {
            reference,
            view,
            live: None,
        }))
    }

    // Counted draws are never deferred: the counter does not depend on
    // the deferred work.
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.reference.clear_pixel_count()
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.reference.report_pixel_count()
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.reference.read_pixel_count(ticket)
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.reference.try_pixel_count(ticket)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.reference.drop_pixel_count(ticket)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<Self::Surface, String> {
        let reference = self
            .reference
            .create_texture_target(format, width, height)?;
        // A level the size of the back buffer (an intermediate drawn with
        // the depth buffer, like Halo's texaccum) scales with it; smaller
        // ones (bloom, reflections, shadows) stay at 1x in the reference.
        let view = if self.frame == Some((width, height)) {
            match self
                .view
                .create_scaled_texture_target(format, width, height, self.scale)
            {
                Ok(v) => Some(v),
                Err(e) => {
                    self.view_failed("render target", e);
                    None
                }
            }
        } else {
            None
        };
        if let Some(v) = view {
            self.scaled.push(Scaled {
                original: v,
                current: v,
                reference,
                size: (width, height),
                format,
                depth: false,
                target: true,
            });
        }
        Ok(ScaledSurface {
            reference,
            view,
            size: (width, height),
        })
    }

    fn can_sample_surfaces(&self) -> bool {
        self.reference.can_sample_surfaces()
    }

    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        self.settle()?;
        let Some(reference) = self.reference.surface_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = match self.live(surface.view) {
            Some(v) => match self.view.surface_texture(v) {
                Ok(t) => t,
                Err(e) => {
                    self.view_failed("surface texture", e);
                    None
                }
            },
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture {
            reference,
            view,
            live: Some(surface.reference),
        }))
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.reference.can_sample_depth_surfaces()
    }

    fn depth_level_texture(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<Self::Texture>, String> {
        // Depth levels (shadow maps) are compared texel by texel: the view
        // samples the reference's 1x bits.
        self.settle()?;
        let Some(reference) = self.reference.depth_level_texture(surface.reference)? else {
            return Ok(None);
        };
        let view = self.share(&reference);
        Ok(Some(ScaledTexture {
            reference,
            view,
            live: None,
        }))
    }

    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.reference.judge()
    }

    fn defers_depth_aliases(&self) -> bool {
        self.reference.defers_depth_aliases()
    }

    #[allow(clippy::too_many_arguments)]
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
        options: DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        self.settle()?;
        let Some(reference) = self.reference.depth_alias_texture(
            depth.reference,
            format,
            width,
            height,
            pitch,
            generation,
            offset,
            desc,
            options,
        )?
        else {
            return Ok(None);
        };
        // The whole buffer as one linear texture of its size (Halo's
        // 0x509368 and 0x50936C, the fog and distortion passes): made from
        // the view's scaled depth, on the GPU. The draw keeps the guest
        // size for texel addressing, so the scaled texture lines up.
        // Anything else is the reference's 1x texture.
        let s = self.scale;
        let whole = offset == 0
            && format == D3DFormat::LinD24S8
            && (desc.width, desc.height) == (width, height)
            && desc.pitch == pitch
            && desc.levels == 1
            && desc.kind == gfx::texture::TextureKind::D2
            && desc.format.layout() == gfx::format::Layout::Linear;
        let mut view = None;
        if let Some(vd) = self
            .live(depth.view)
            .filter(|_| whole && self.view.defers_depth_aliases())
        {
            let scaled = TextureDesc {
                width: desc.width * s,
                height: desc.height * s,
                pitch: desc.pitch * s,
                ..desc.clone()
            };
            match self.view.depth_alias_texture(
                vd,
                format,
                width * s,
                height * s,
                pitch * s,
                generation,
                0,
                &scaled,
                options,
            ) {
                Ok(Some(t)) => {
                    self.stats.scaled_depth_aliases += 1;
                    view = Some(t);
                }
                Ok(None) => {}
                Err(e) => self.view_failed("depth alias", e),
            }
        }
        let view = match view {
            Some(v) => Some(v),
            None => self.share(&reference),
        };
        Ok(Some(ScaledTexture {
            reference,
            view,
            live: None,
        }))
    }

    fn can_mirror(&self) -> bool {
        self.reference.can_mirror()
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.settle()?;
        self.reference.mirror_reserve(len)
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.settle()?;
        self.reference.mirror_write(offset, bytes)
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
        self.settle()?;
        self.reference
            .mirror_store(surface.reference, format, width, height, layout, offset)
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
        self.settle()?;
        self.before_shared_write(&surface);
        self.reference
            .mirror_load(surface.reference, format, width, height, layout, offset)?;
        self.refill(&surface);
        Ok(())
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.settle()?;
        self.reference.mirror_read(offset, len)
    }
}

/// `bytes`, `width x height` texels of `bpp` bytes in tight rows, with each
/// texel repeated into a `scale x scale` block.
fn repeat_texels(bytes: &[u8], width: u32, height: u32, bpp: u32, scale: u32) -> Vec<u8> {
    let (w, h, bpp, s) = (
        width as usize,
        height as usize,
        bpp as usize,
        scale as usize,
    );
    let mut out = Vec::with_capacity(bytes.len() * s * s);
    for y in 0..h {
        let row = &bytes[y * w * bpp..(y + 1) * w * bpp];
        let start = out.len();
        for texel in row.chunks_exact(bpp) {
            for _ in 0..s {
                out.extend_from_slice(texel);
            }
        }
        let end = out.len();
        for _ in 1..s {
            out.extend_from_within(start..end);
        }
    }
    out
}

impl ScaleView<GfxBackend> for GfxBackend {
    fn create_scaled_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        let (w, h) = (width.checked_mul(scale), height.checked_mul(scale));
        let (Some(w), Some(h)) = (w, h) else {
            return Err(format!("{width}x{height} at {scale}x overflows"));
        };
        let s = self.create_surface(format, w, h, depth)?;
        if scale > 1 {
            self.scales.insert(s, scale);
        }
        Ok(s)
    }

    fn create_scaled_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        scale: u32,
    ) -> Result<gfx::surface::SurfaceId, String> {
        let (w, h) = (width.checked_mul(scale), height.checked_mul(scale));
        let (Some(w), Some(h)) = (w, h) else {
            return Err(format!("{width}x{height} at {scale}x overflows"));
        };
        let s = self.create_texture_target(format, w, h)?;
        if scale > 1 {
            self.scales.insert(s, scale);
        }
        Ok(s)
    }

    fn stop_counting(&mut self) {
        self.count_samples = false;
    }

    fn share_texture(
        &mut self,
        reference: &GfxBackend,
        texture: &HostTexture,
    ) -> Option<HostTexture> {
        (reference.ctx.device == self.ctx.device).then(|| texture.shared())
    }

    fn submit_recorded(&mut self) -> Result<(), String> {
        self.flush()
    }

    fn retain_reference_texture(&self, texture: &HostTexture) -> Option<(HostTexture, bool)> {
        Some((texture.shared(), texture.has_view()))
    }

    fn reference_present_reads(&self, reference: &GfxBackend) -> bool {
        reference.has_present_hook()
    }

    fn fill_from(
        &mut self,
        reference: &mut GfxBackend,
        src: gfx::surface::SurfaceId,
        dst: gfx::surface::SurfaceId,
    ) -> Result<bool, String> {
        use gfx::surface::{SurfaceError, SurfaceKind, SurfaceLayout};
        reference.flush()?;
        self.flush()?;
        let s = reference.store.get(src).map_err(|e| e.to_string())?;
        let d = self.store.get(dst).map_err(|e| e.to_string())?;
        match (s.kind(), d.kind()) {
            (SurfaceKind::Color, SurfaceKind::Color) => {}
            (SurfaceKind::DepthStencil, SurfaceKind::DepthStencil) => {
                // Read the reference's words back, repeat each, and draw
                // them into the scaled depth (a guest write's path).
                let (sd, dd) = (*s.desc(), *d.desc());
                let scale = dd.width / sd.width.max(1);
                if scale == 0 || (sd.width * scale, sd.height * scale) != (dd.width, dd.height) {
                    return Err(format!(
                        "depth fill from {}x{} into {}x{}",
                        sd.width, sd.height, dd.width, dd.height
                    ));
                }
                let format = match sd.format {
                    D3DFormat::D24S8 | D3DFormat::LinD24S8 => D3DFormat::LinD24S8,
                    D3DFormat::D16 | D3DFormat::LinD16 => D3DFormat::LinD16,
                    // Float depth: no guest words to carry it.
                    _ => return Ok(false),
                };
                let bpp = format.bytes_per_texel().unwrap_or(4);
                let layout = SurfaceLayout::Linear {
                    pitch: sd.width * bpp,
                };
                let bytes =
                    match reference
                        .store
                        .read_depth_as(&reference.ctx, src, format, &layout)
                    {
                        Ok(b) => b,
                        Err(SurfaceError::NotReadable(..)) => return Ok(false),
                        Err(e) => return Err(e.to_string()),
                    };
                reference.readbacks += 1;
                let words = repeat_texels(&bytes, sd.width, sd.height, bpp, scale);
                let desc = TextureDesc::new(
                    format,
                    gfx::texture::TextureKind::D2,
                    dd.width,
                    dd.height,
                    1,
                    1,
                );
                self.store
                    .write_depth(&self.ctx, dst, &desc, &words)
                    .map_err(|e| e.to_string())?;
                return Ok(true);
            }
            (a, b) => return Err(format!("fill of a {b:?} surface from a {a:?} one")),
        }
        let (src_view, dst_view, format) = (s.view().clone(), d.view().clone(), d.wgpu_format());
        let ctx = &self.ctx;
        let blitter = self
            .blitter
            .get_or_insert_with(|| gfx::blit::Blitter::new(ctx));
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        // Nearest: each guest pixel becomes a scale x scale block.
        blitter.blit(
            ctx,
            &mut enc,
            &src_view,
            &dst_view,
            format,
            gfx::blit::BlitOptions::default(),
        );
        ctx.queue.submit([enc.finish()]);
        Ok(true)
    }

    fn present_reference(
        &mut self,
        reference: &GfxBackend,
        surface: gfx::surface::SurfaceId,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.present_from(Some(reference), surface, flags, gamma)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_takes_whole_scales_from_one_to_six() {
        assert_eq!(render_scale_from(None), Ok(1));
        assert_eq!(render_scale_from(Some("")), Ok(1));
        assert_eq!(render_scale_from(Some("1")), Ok(1));
        assert_eq!(render_scale_from(Some(" 4 ")), Ok(4));
        assert_eq!(render_scale_from(Some("6")), Ok(6));
        for bad in ["0", "7", "1.5", "2x", "-1"] {
            assert!(render_scale_from(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn scale_one_builds_no_view() {
        assert!(view_for(1, || -> u32 { panic!("a view at 1x") }).is_none());
        assert_eq!(view_for(2, || 7u32), Some(7));
    }

    #[test]
    fn texels_repeat_into_blocks() {
        // 2x1 texels of 2 bytes at 3x: 6x3 texels.
        let out = repeat_texels(&[1, 2, 3, 4], 2, 1, 2, 3);
        let row = [1, 2, 1, 2, 1, 2, 3, 4, 3, 4, 3, 4];
        assert_eq!(out, [row, row, row].concat());
        assert_eq!(repeat_texels(&[9], 1, 1, 1, 1), [9]);
    }
}
