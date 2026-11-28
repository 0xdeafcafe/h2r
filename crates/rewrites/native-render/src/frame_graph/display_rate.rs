//! Display frames between sealed scenes (docs/graphics/framescene.md,
//! section 7 and slice 8).
//!
//! [`Display`] holds the render thread's [`DisplayLoop`] over the last two
//! frames the owner drew. At a frame's present the loop decides whether
//! the window shows that frame or a blend toward it from the frame before
//! ([`Display::at_swap`]); while nothing is queued the frame owner asks it
//! for the frames after ([`Display::idle`]). A display frame is the newer
//! frame drawn again with each view's camera blended
//! (`frame_scene::blend`), which the graph hands its stages.
//!
//! Each display frame is presented as `gfx::present_camera::Present::Synthetic`
//! ([`marks_present`]): the window projects the world marks of the game's
//! image with the cameras the frame drew its views with
//! (docs/game/high-framerate.md, "World marks on made frames").

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use frame_scene::Camera;
use frame_scene::blend::Plan;
use frame_scene::display::{AtSwap, DisplayLoop, DisplayStats, Drawn, Idle};
use frame_scene::pace::Rate;
use gfx::present_camera::{self, MarkShifts, Present};

use crate::renderer::Renderer;
use crate::stage::Frame;

/// What the loop keeps of a drawn frame to draw it again.
#[derive(Clone)]
pub struct Kept {
    pub frame: Arc<Mutex<Frame>>,
    /// The game's image at its present: its world marks and their cameras.
    pub image: Option<Arc<present_camera::Image>>,
}

impl Kept {
    pub fn new(frame: Frame) -> Self {
        let image = match &frame.present {
            Present::Guest { image, .. } => image.clone(),
            Present::Synthetic { source, .. } => source.clone(),
        };
        Self { frame: Arc::new(Mutex::new(frame)), image }
    }

    fn lock(&self) -> MutexGuard<'_, Frame> {
        self.frame.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// `render.stats`' `display_rate`: the loop's counters and the marks the
/// display frames moved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayRateStats {
    pub frames: DisplayStats,
    /// Views of the marks' image on display frames whose marks were moved
    /// onto the camera the frame drew the view with.
    pub marks_moved: u64,
    /// Views of the marks' image on display frames whose marks kept the
    /// image's camera: no drawn view at their viewport, or a camera that
    /// cannot be inverted.
    pub marks_held: u64,
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
    fn moved(&mut self, marks: MarkShifts) {
        self.stats.marks_moved += u64::from(marks.moved);
        self.stats.marks_held += u64::from(marks.held);
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

/// What the frame owner does on an idle turn.
pub enum IdleTurn {
    /// Nothing until the next frame.
    Stop,
    /// Ask again at this time.
    Wait(Instant),
    /// A display frame is drawn: present it with this metadata, then ask
    /// again at once.
    Drawn(Present),
}

/// The display-rate loop the frame owner drives.
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

    /// A new frame at its present: draws it on `r`, or the blend toward it
    /// the loop asks for. Returns the metadata to present it with.
    pub fn at_swap(&self, r: &mut Renderer, frame: Kept) -> Present {
        let started = Instant::now();
        let mut state = self.lock();
        let rate = (state.rate)();
        let scene = Arc::clone(&frame.lock().scene);
        match state.frames.swap(scene, frame.clone(), started, rate) {
            AtSwap::Newest => {
                let mut f = frame.lock();
                r.draw(&mut f, None);
                f.present.clone()
            }
            AtSwap::Blend { a, alpha } => {
                let (_, cameras) = r.draw(&mut frame.lock(), Some((&*a.scene, alpha)));
                let plan = Plan::Blend(alpha);
                let (marks, shifts) = marks_present(a.frame.image.as_ref(), frame.image.as_ref(), plan, &cameras.drawn());
                state.moved(shifts);
                state.frames.drawn(Drawn::Swap, started.elapsed());
                marks
            }
        }
    }

    /// Nothing is queued at `now`: draws the display frame that is due on
    /// `r`, if any.
    pub fn idle(&self, r: &mut Renderer, now: Instant) -> IdleTurn {
        let mut state = self.lock();
        let rate = (state.rate)();
        match state.frames.idle(now, rate) {
            Idle::Stop => IdleTurn::Stop,
            Idle::Wait(at) => IdleTurn::Wait(at),
            Idle::Draw { a, b, plan } => {
                let started = Instant::now();
                let from = match plan {
                    Plan::Blend(alpha) => Some((&*a.scene, alpha)),
                    Plan::Newest => None,
                };
                let (_, cameras) = r.draw(&mut b.frame.lock(), from);
                let (marks, shifts) = marks_present(a.frame.image.as_ref(), b.frame.image.as_ref(), plan, &cameras.drawn());
                state.moved(shifts);
                state.frames.drawn(Drawn::Between(plan.into()), started.elapsed());
                IdleTurn::Drawn(marks)
            }
        }
    }

    /// The timeline changed: the next frame does not follow on from what
    /// is on screen.
    pub fn reset(&self) {
        self.lock().frames.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::present_camera::{Image, View, ViewShift};

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
        display.lock().moved(MarkShifts { moved: 2, held: 1 });
        display.lock().moved(MarkShifts { moved: 1, held: 0 });
        let stats = display.lock().stats;
        assert_eq!((stats.marks_moved, stats.marks_held), (3, 1));
    }
}
