//! Display frames between sealed scenes on the frame owner's device
//! (docs/graphics/framescene.md, section 7 and slice 8).
//!
//! [`Display`] holds the render thread's [`DisplayLoop`] over the last two
//! scenes the owner drew. At an owned present the loop decides whether the
//! window shows that scene or a blend toward it from the scene before
//! ([`Display::at_swap`]); while nothing is queued the render thread asks
//! it for the frames after ([`Display::idle`], through
//! [`d3d::threaded::owner::DisplayRate`]). A display frame is the newer
//! scene's frame drawn on its own targets with each view's camera blended
//! (`frame_scene::blend`, moved onto the draws by
//! `d3d::scene_sink::Retarget`) and shown as a host-only present. It
//! never touches the guest's attachments, tickets or sampled textures.
//!
//! Each display frame is presented as `d3d::present_camera::Present::Synthetic`
//! ([`marks_present`]): the window projects the world marks of the guest's
//! image with the cameras the frame drew its views with
//! (docs/game/high-framerate.md, "World marks on made frames").

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use d3d::gfx_backend::GuestDrawPass;
use d3d::present_camera::{self, MarkShifts, Present};
use d3d::scaled::ScaledBackend;
use d3d::scaled::private_scene::MixedPrivatePass;
use d3d::scene_sink::{ReplayFrame, Retarget, RetargetCounts};
use d3d::threaded::owner::DisplayRate;
use d3d::{GfxBackend, Lighting};
use frame_scene::{Camera, FrameScene, ViewKind};
use frame_scene::blend::{self, Plan};
use frame_scene::display::{AtSwap, DisplayLoop, DisplayStats, Drawn, Idle};
use frame_scene::pace::Rate;
use gfx::surface::SurfaceId;

use super::{record, sun_tiles};

type Scaled = ScaledBackend<GfxBackend, GfxBackend>;

/// What the loop keeps of a drawn scene to draw it again.
#[derive(Clone)]
pub struct Kept {
    pub replay: Arc<ReplayFrame>,
    /// The scale of the targets its display frames draw on.
    pub scale: u32,
    /// The guest's image at its present: its world marks and their cameras
    /// (`d3d::present_camera`).
    pub image: Option<Arc<present_camera::Image>>,
}

/// `render.stats`' `display_rate`: the loop's counters and what the
/// display frames moved onto the blended cameras.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayRateStats {
    pub frames: DisplayStats,
    /// Carried draws drawn on a blended camera, over all display frames.
    pub moved_draws: u64,
    /// Typed stages drawn on a blended camera.
    pub moved_stages: u64,
    /// Typed stages in a game view drawn as their tick drew them.
    pub held_stages: u64,
    /// Views of the marks' image on display frames whose marks were moved
    /// onto the camera the frame drew the view with.
    pub marks_moved: u64,
    /// Views of the marks' image on display frames whose marks kept the
    /// image's camera: no drawn view at their viewport, or a camera that
    /// cannot be inverted.
    pub marks_held: u64,
}

/// A display frame drawn by [`draw_blend`].
pub struct DisplayFrame {
    /// The presented target, which the caller shows.
    pub image: SurfaceId,
    /// What moved onto the blended cameras.
    pub moved: RetargetCounts,
    /// Each game view's camera as drawn, in the scene's order (first-person
    /// views left out: they share their game view's viewport).
    pub cameras: Vec<Camera>,
}

/// A display frame the window is shown at a Swap in place of its scene.
pub struct Shown {
    pub image: SurfaceId,
    /// The present's metadata: present the image inside
    /// `present_camera::with(Some(marks), ..)`.
    pub marks: Present,
}

/// The present metadata of a display frame drawn at `plan` from `a` toward
/// `b` with `cameras` (its game views'): the marks come from `b`'s image,
/// or from `a`'s while the blend is under one half, so what they show lags
/// the world by at most half a tick; each view's camera is moved onto the
/// one the frame drew it with.
pub fn marks_present(
    a: Option<&Arc<present_camera::Image>>,
    b: Option<&Arc<present_camera::Image>>,
    plan: Plan,
    cameras: &[Camera],
) -> (Present, MarkShifts) {
    let fraction = match plan {
        Plan::Blend(alpha) => alpha,
        Plan::Newest => 1.0,
    };
    let source = if fraction < 0.5 { a } else { b };
    Present::display_frame(source.cloned(), cameras, fraction)
}

struct State {
    frames: DisplayLoop<Kept>,
    stats: DisplayRateStats,
    /// The frame-rate setting (`graphics.fps`) as a display rate.
    rate: Box<dyn Fn() -> Rate + Send>,
}

impl State {
    fn moved(&mut self, counts: RetargetCounts, marks: MarkShifts) {
        self.stats.moved_draws += u64::from(counts.draws);
        self.stats.moved_stages += u64::from(counts.stages);
        self.stats.held_stages += u64::from(counts.held_stages);
        self.stats.marks_moved += u64::from(marks.moved);
        self.stats.marks_held += u64::from(marks.held);
    }

    fn failed(&mut self, error: &str) {
        if self.frames.stats().failed == 0 {
            eprintln!("display rate: a display frame failed ({error}); the window keeps the last one");
        }
        self.frames.failed();
    }
}

/// The installed loop `render.stats` reads.
static ACTIVE: Mutex<Weak<Mutex<State>>> = Mutex::new(Weak::new());

/// The installed loop's counters; `None` without a frame owner.
pub fn stats() -> Option<DisplayRateStats> {
    let state = ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).upgrade()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    Some(DisplayRateStats { frames: state.frames.stats(), ..state.stats })
}

/// The display-rate loop shared by an owner's presents and its render
/// thread's idle turns.
#[derive(Clone)]
pub struct Display {
    state: Arc<Mutex<State>>,
}

impl Display {
    /// A loop paced by `rate`, read at every present and idle turn. It is
    /// the one `render.stats` reports from then on.
    pub fn new(rate: impl Fn() -> Rate + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(State {
            frames: DisplayLoop::new(Instant::now()),
            stats: DisplayRateStats::default(),
            rate: Box::new(rate),
        }));
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&state);
        Self { state }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The render thread's side over a 1x display, with the owner's
    /// lighting setting.
    pub fn on_display(&self, lighting: Arc<dyn Fn() -> Lighting + Send + Sync>) -> Box<dyn DisplayRate<GfxBackend>> {
        Box::new(OnDisplay { display: self.clone(), lighting })
    }

    /// The render thread's side over a scaled display: the view draws.
    pub fn on_scaled(&self) -> Box<dyn DisplayRate<Scaled>> {
        Box::new(OnScaled { display: self.clone() })
    }

    /// An owned present of `scene` after its frame was recorded on the
    /// live attachments, on the render thread inside its present's
    /// metadata (the guest's image, `present_camera::guest_image`).
    /// Returns the display frame the window shows in its place, drawn on
    /// `gpu`'s own targets at `scale`; `None` shows the scene itself.
    pub fn at_swap(
        &self,
        gpu: &mut GfxBackend,
        scene: &Arc<FrameScene>,
        replay: &Arc<ReplayFrame>,
        scale: u32,
        lighting: Lighting,
    ) -> Option<Shown> {
        let started = Instant::now();
        let mut state = self.lock();
        let image = present_camera::guest_image();
        let kept = Kept { replay: Arc::clone(replay), scale, image: image.clone() };
        let rate = (state.rate)();
        let AtSwap::Blend { a, alpha } = state.frames.swap(Arc::clone(scene), kept, started, rate) else {
            return None;
        };
        let plan = Plan::Blend(alpha);
        match draw_blend(gpu, &a.scene, scene, replay, scale, lighting, plan) {
            Ok(frame) => {
                let (marks, shifts) = marks_present(a.frame.image.as_ref(), image.as_ref(), plan, &frame.cameras);
                state.moved(frame.moved, shifts);
                state.frames.drawn(Drawn::Swap, started.elapsed());
                Some(Shown { image: frame.image, marks })
            }
            Err(e) => {
                state.failed(&e);
                None
            }
        }
    }

    /// The render thread has nothing queued at `now`: draws the display
    /// frame that is due, if any, and returns when to ask again.
    pub fn idle(&self, gpu: &mut GfxBackend, lighting: Lighting, now: Instant) -> Option<Instant> {
        let mut state = self.lock();
        let rate = (state.rate)();
        match state.frames.idle(now, rate) {
            Idle::Stop => None,
            Idle::Wait(at) => Some(at),
            Idle::Draw { a, b, plan } => {
                let started = Instant::now();
                let Kept { replay, scale, image } = &b.frame;
                let shown = draw_blend(gpu, &a.scene, &b.scene, replay, *scale, lighting, plan)
                    .and_then(|frame| {
                        let (marks, shifts) = marks_present(a.frame.image.as_ref(), image.as_ref(), plan, &frame.cameras);
                        present_camera::with(Some(marks), || gpu.show_between(frame.image, replay.gamma.as_deref()))?;
                        Ok((frame.moved, shifts))
                    });
                match shown {
                    Ok((moved, shifts)) => {
                        state.moved(moved, shifts);
                        state.frames.drawn(Drawn::Between(plan.into()), started.elapsed());
                        // The next slot: the loop answers with its time.
                        Some(Instant::now())
                    }
                    Err(e) => {
                        state.failed(&e);
                        None
                    }
                }
            }
        }
    }

    /// A Swap went out without the graph, or the timeline changed.
    pub fn reset(&self) {
        self.lock().frames.reset();
    }
}

struct OnDisplay {
    display: Display,
    lighting: Arc<dyn Fn() -> Lighting + Send + Sync>,
}

impl DisplayRate<GfxBackend> for OnDisplay {
    fn idle(&mut self, gpu: &mut GfxBackend, now: Instant) -> Option<Instant> {
        self.display.idle(gpu, (self.lighting)(), now)
    }
    fn declined(&mut self) {
        self.display.reset();
    }
}

struct OnScaled {
    display: Display,
}

impl DisplayRate<Scaled> for OnScaled {
    fn idle(&mut self, backend: &mut Scaled, now: Instant) -> Option<Instant> {
        let lighting = backend.lighting();
        self.display.idle(backend.view_mut(), lighting, now)
    }
    fn declined(&mut self) {
        self.display.reset();
    }
}

/// `b`'s frame drawn on `gpu`'s own targets at `scale`: with each view's
/// camera blended from `a` for [`Plan::Blend`], as `b` itself for
/// [`Plan::Newest`]. Above 1x a frame with fixed-size attachments beside
/// the display grid draws them at their own size, as a private capture
/// does (`MixedPrivatePass`), with no sun tile.
pub fn draw_blend(
    gpu: &mut GfxBackend,
    a: &FrameScene,
    b: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
    plan: Plan,
) -> Result<DisplayFrame, String> {
    let blended = match plan {
        Plan::Blend(alpha) => Some(blend::blend(a, b, alpha)),
        Plan::Newest => None,
    };
    let mut retarget = blended.as_ref().map(|blended| {
        let to = b.views.iter().map(|v| v.key).zip(blended.cameras.iter());
        Retarget::new(b.view_cameras(), to)
    });
    let image = if scale > 1 && replay.has_mixed_target_sizes() {
        let mut pass = MixedPrivatePass::new_lit(gpu, replay, scale, lighting)?;
        record(&mut pass, b, replay, true, lighting, None, None, retarget.as_mut())?;
        pass.into_image(replay.presented)?
    } else {
        let sun_replay = sun_tiles(&gpu.ctx.device, b, replay, scale, lighting);
        let mut pass = GuestDrawPass::new_lit(gpu, replay, scale, lighting)?;
        record(&mut pass, b, replay, true, lighting, None, sun_replay, retarget.as_mut())?;
        pass.into_image()?
    };
    let drawn = match &blended {
        Some(blended) => &blended.cameras[..],
        None => &[][..],
    };
    let cameras = b
        .views
        .iter()
        .enumerate()
        .filter(|(_, v)| v.key.kind != ViewKind::FirstPerson)
        .map(|(i, v)| drawn.get(i).copied().unwrap_or(v.camera))
        .collect();
    Ok(DisplayFrame { image, moved: retarget.map(|r| r.counts()).unwrap_or_default(), cameras })
}

#[cfg(test)]
mod tests {
    use super::*;
    use d3d::present_camera::{Image, View, ViewShift};

    fn camera(yaw: f32) -> Camera {
        Camera {
            position: [0.0, 0.0, 1.0],
            forward: [yaw.cos(), yaw.sin(), 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.2,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.05,
            far: 500.0,
        }
    }

    fn image(yaw: f32) -> Arc<Image> {
        let view = View { camera: camera(yaw), horizontal_factor: 1.0 };
        Arc::new(Image { views: [Some(view), None, None, None], payload: Arc::new(()) })
    }

    fn synthetic(present: Present) -> (Arc<Image>, Vec<ViewShift>, f32) {
        let Present::Synthetic { source, shifts, fraction, .. } = present else {
            panic!("a display frame's present is synthetic")
        };
        (source.expect("an image"), shifts, fraction)
    }

    #[test]
    fn early_in_a_blend_the_marks_come_from_the_tick_before_moved_onto_the_blend() {
        let (a, b) = (image(0.3), image(0.5));
        let drawn = camera(0.35);
        let (present, counts) = marks_present(Some(&a), Some(&b), Plan::Blend(0.25), &[drawn]);
        assert_eq!(counts, MarkShifts { moved: 1, held: 0 });
        let (source, shifts, fraction) = synthetic(present);
        assert!(Arc::ptr_eq(&source, &a));
        assert_eq!(fraction, 0.25);
        assert_eq!(shifts, [ViewShift::between(&camera(0.3), &drawn).unwrap()]);
    }

    #[test]
    fn late_in_a_blend_the_marks_come_from_the_newest_tick() {
        let (a, b) = (image(0.3), image(0.5));
        let (present, counts) = marks_present(Some(&a), Some(&b), Plan::Blend(0.75), &[camera(0.45)]);
        assert_eq!(counts, MarkShifts { moved: 1, held: 0 });
        let (source, shifts, _) = synthetic(present);
        assert!(Arc::ptr_eq(&source, &b));
        assert_eq!(shifts, [ViewShift::between(&camera(0.5), &camera(0.45)).unwrap()]);
    }

    #[test]
    fn the_newest_scene_drawn_as_it_is_keeps_its_marks_where_they_are() {
        let (a, b) = (image(0.3), image(0.5));
        let (present, counts) = marks_present(Some(&a), Some(&b), Plan::Newest, &[camera(0.5)]);
        assert_eq!(counts, MarkShifts::default());
        let (source, shifts, fraction) = synthetic(present);
        assert!(Arc::ptr_eq(&source, &b));
        assert!(shifts.is_empty());
        assert_eq!(fraction, 1.0);
    }

    #[test]
    fn display_frame_marks_count_into_the_stats() {
        let display = Display::new(|| Rate::Off);
        display.lock().moved(RetargetCounts::default(), MarkShifts { moved: 2, held: 1 });
        display.lock().moved(RetargetCounts::default(), MarkShifts { moved: 1, held: 0 });
        let stats = display.lock().stats;
        assert_eq!((stats.marks_moved, stats.marks_held), (3, 1));
    }
}
