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

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use d3d::gfx_backend::GuestDrawPass;
use d3d::scaled::ScaledBackend;
use d3d::scene_sink::{ReplayFrame, Retarget, RetargetCounts};
use d3d::threaded::owner::DisplayRate;
use d3d::{GfxBackend, Lighting};
use frame_scene::FrameScene;
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
}

struct State {
    frames: DisplayLoop<Kept>,
    stats: DisplayRateStats,
    /// The frame-rate setting (`graphics.fps`) as a display rate.
    rate: Box<dyn Fn() -> Rate + Send>,
}

impl State {
    fn moved(&mut self, counts: RetargetCounts) {
        self.stats.moved_draws += u64::from(counts.draws);
        self.stats.moved_stages += u64::from(counts.stages);
        self.stats.held_stages += u64::from(counts.held_stages);
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
    /// live attachments. Returns the display frame the window shows in
    /// its place, drawn on `gpu`'s own targets at `scale`; `None` shows
    /// the scene itself.
    pub fn at_swap(
        &self,
        gpu: &mut GfxBackend,
        scene: &Arc<FrameScene>,
        replay: &Arc<ReplayFrame>,
        scale: u32,
        lighting: Lighting,
    ) -> Option<SurfaceId> {
        let started = Instant::now();
        let mut state = self.lock();
        let kept = Kept { replay: Arc::clone(replay), scale };
        let rate = (state.rate)();
        let AtSwap::Blend { a, alpha } = state.frames.swap(Arc::clone(scene), kept, started, rate) else {
            return None;
        };
        match draw_blend(gpu, &a.scene, scene, replay, scale, lighting, Plan::Blend(alpha)) {
            Ok((image, moved)) => {
                state.moved(moved);
                state.frames.drawn(Drawn::Swap, started.elapsed());
                Some(image)
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
                let Kept { replay, scale } = &b.frame;
                let shown = draw_blend(gpu, &a.scene, &b.scene, replay, *scale, lighting, plan)
                    .and_then(|(image, moved)| {
                        gpu.show_between(image, replay.gamma.as_deref())?;
                        Ok(moved)
                    });
                match shown {
                    Ok(moved) => {
                        state.moved(moved);
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
/// [`Plan::Newest`]. Returns the presented target, which the caller
/// shows, and what moved onto the blended cameras.
pub fn draw_blend(
    gpu: &mut GfxBackend,
    a: &FrameScene,
    b: &FrameScene,
    replay: &ReplayFrame,
    scale: u32,
    lighting: Lighting,
    plan: Plan,
) -> Result<(SurfaceId, RetargetCounts), String> {
    if scale > 1 && replay.has_mixed_target_sizes() {
        return Err("fixed-size attachments beside the display grid".into());
    }
    let blended = match plan {
        Plan::Blend(alpha) => Some(blend::blend(a, b, alpha)),
        Plan::Newest => None,
    };
    let mut retarget = blended.as_ref().map(|blended| {
        let to = b.views.iter().map(|v| v.key).zip(blended.cameras.iter());
        Retarget::new(b.view_cameras(), to)
    });
    let sun_replay = sun_tiles(&gpu.ctx.device, b, replay, scale, lighting);
    let mut pass = GuestDrawPass::new_lit(gpu, replay, scale, lighting)?;
    record(&mut pass, b, replay, true, lighting, None, sun_replay, retarget.as_mut())?;
    let image = pass.into_image()?;
    Ok((image, retarget.map(|r| r.counts()).unwrap_or_default()))
}
