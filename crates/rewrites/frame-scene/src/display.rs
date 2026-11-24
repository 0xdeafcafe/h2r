//! The render thread's display-rate loop (docs/graphics/framescene.md,
//! section 7 and slice 8): which display frames are drawn between two
//! taken scenes, and at what alpha.
//!
//! Pure: the caller passes the clock and the frame-rate setting, draws
//! what [`DisplayLoop`] asks for and reports how it went. A scene's own
//! present ([`DisplayLoop::swap`]) shows a blend toward it from the scene
//! before (alpha near 0, the one tick of delay); the frames after it
//! ([`DisplayLoop::idle`]) step on the display grid until alpha reaches 1,
//! where the scene itself is drawn once and the loop waits for the next.
//! [`crate::pace`] has the timing rules and [`crate::blend`] the values.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::blend::{self, Plan};
use crate::pace::{self, Headroom, Rate, Schedule};
use crate::scene::FrameScene;

/// A taken scene and what the renderer keeps to draw it again (its
/// carried frame, its scale).
pub struct Taken<T> {
    pub scene: Arc<FrameScene>,
    pub frame: T,
}

impl<T: Clone> Clone for Taken<T> {
    fn clone(&self) -> Self {
        Self { scene: Arc::clone(&self.scene), frame: self.frame.clone() }
    }
}

/// What a scene's own present shows.
pub enum AtSwap<T> {
    /// The scene itself: 30 fps, the first scene, or a pair that
    /// [`blend::plan`] does not blend (a cut, a pause, a long gap).
    Newest,
    /// A blend from `a` toward the scene just taken, `0 <= alpha < 1`.
    Blend { a: Taken<T>, alpha: f32 },
}

/// What the render thread does while nothing is queued.
pub enum Idle<T> {
    /// Nothing until the next scene's present.
    Stop,
    /// Ask again at this time.
    Wait(Instant),
    /// Draw this display frame now, as a host-only present: a blend
    /// from `a` toward `b`, or `b` itself once alpha reaches 1.
    Draw { a: Taken<T>, b: Taken<T>, plan: Plan },
}

/// A display frame the caller drew, for [`DisplayLoop::drawn`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// A scene's own present, shown as a blend ([`AtSwap::Blend`]).
    Swap,
    /// A frame between presents, from [`Idle::Draw`].
    Between(PlanKind),
}

/// [`Plan`] without its alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    Blend,
    Newest,
}

impl From<Plan> for PlanKind {
    fn from(p: Plan) -> Self {
        match p {
            Plan::Blend(_) => PlanKind::Blend,
            Plan::Newest => PlanKind::Newest,
        }
    }
}

/// The loop's counters since it started (`render.stats`' `display_rate`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct DisplayStats {
    /// The rate the setting asks for; 0 at 30 or below.
    pub rate_hz: u32,
    /// The rate drawn at after the headroom cap ([`pace::Headroom`]).
    pub shown_hz: u32,
    /// Rates above 60 are held back because display frames do not fit.
    pub capped: bool,
    /// Scenes' own presents that went through the loop.
    pub swaps: u64,
    /// Display frames drawn as a blend: swaps and frames between them.
    pub blended: u64,
    /// Of `blended`, the scenes' own presents.
    pub swap_blends: u64,
    /// Frames between presents drawn as the newest scene (alpha reached 1).
    pub between_newest: u64,
    /// Display frames that failed to draw; the window kept what it had.
    pub failed: u64,
    /// Pairs broken by a present without the graph or a timeline change.
    pub resets: u64,
    /// The average render-thread cost of one display frame, microseconds.
    pub avg_cost_us: u64,
}

pub struct DisplayLoop<T> {
    base: Instant,
    older: Option<Taken<T>>,
    newer: Option<Taken<T>>,
    /// The newer pair still has frames to draw: the last alpha shown was
    /// below 1.
    blending: bool,
    schedule: Schedule,
    headroom: Headroom,
    stats: DisplayStats,
}

fn hz(rate: Rate) -> u32 {
    match rate {
        Rate::Off => 0,
        Rate::Hz(h) => h,
    }
}

impl<T: Clone> DisplayLoop<T> {
    /// Times are kept as nanoseconds after `base`; anything earlier reads
    /// as `base`.
    pub fn new(base: Instant) -> Self {
        Self {
            base,
            older: None,
            newer: None,
            blending: false,
            schedule: Schedule::default(),
            headroom: Headroom::default(),
            stats: DisplayStats::default(),
        }
    }

    fn ns(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.base).as_nanos() as u64
    }

    fn at(&self, ns: u64) -> Instant {
        self.base + Duration::from_nanos(ns)
    }

    /// The display period after the headroom cap; `None` at 30.
    fn period(&mut self, wanted: Rate) -> Option<u64> {
        let shown = self.headroom.cap(wanted);
        self.stats.rate_hz = hz(wanted);
        self.stats.shown_hz = hz(shown);
        self.stats.capped = self.headroom.capped();
        shown.period_ns()
    }

    /// A scene was taken for its own present at `now`. Returns what that
    /// present shows; the caller reports a blend with [`Self::drawn`].
    pub fn swap(&mut self, scene: Arc<FrameScene>, frame: T, now: Instant, wanted: Rate) -> AtSwap<T> {
        self.stats.swaps += 1;
        self.older = self.newer.take();
        self.newer = Some(Taken { scene, frame });
        self.blending = false;
        let Some(period) = self.period(wanted) else {
            return AtSwap::Newest;
        };
        let (Some(a), Some(b)) = (&self.older, &self.newer) else {
            return AtSwap::Newest;
        };
        match blend::plan(&a.scene, &b.scene, now) {
            Plan::Blend(alpha) => {
                let a = a.clone();
                self.blending = true;
                let t = self.ns(now);
                self.schedule.shown_at(t, period);
                AtSwap::Blend { a, alpha }
            }
            Plan::Newest => AtSwap::Newest,
        }
    }

    /// The render thread has nothing queued at `now`.
    pub fn idle(&mut self, now: Instant, wanted: Rate) -> Idle<T> {
        if !self.blending {
            return Idle::Stop;
        }
        let Some(period) = self.period(wanted) else {
            self.blending = false;
            return Idle::Stop;
        };
        let (Some(a), Some(b)) = (&self.older, &self.newer) else {
            self.blending = false;
            return Idle::Stop;
        };
        let (Some(pa), Some(pb)) = (a.scene.stamp.published, b.scene.stamp.published) else {
            self.blending = false;
            return Idle::Stop;
        };
        let t = self.ns(now);
        if !self.schedule.is_due(t) {
            let next = self.schedule.deadline().unwrap_or(t);
            return Idle::Wait(self.at(next));
        }
        // A slot just before the next scene is due would show alpha ~1
        // right before that scene's present shows alpha ~0: wait for it.
        if let Some(wait) = pace::defer(t, self.ns(pa), self.ns(pb), period) {
            return Idle::Wait(self.at(wait));
        }
        self.schedule.due(t, period);
        let plan = blend::plan(&a.scene, &b.scene, now);
        let draw = Idle::Draw { a: a.clone(), b: b.clone(), plan };
        if plan == Plan::Newest {
            self.blending = false;
        }
        draw
    }

    /// The caller drew `what` in `cost` of render-thread time.
    pub fn drawn(&mut self, what: Drawn, cost: Duration) {
        self.headroom.record(cost.as_nanos() as u64);
        self.stats.avg_cost_us = self.headroom.average_ns().unwrap_or(0) / 1_000;
        match what {
            Drawn::Swap => {
                self.stats.blended += 1;
                self.stats.swap_blends += 1;
            }
            Drawn::Between(PlanKind::Blend) => self.stats.blended += 1,
            Drawn::Between(PlanKind::Newest) => self.stats.between_newest += 1,
        }
    }

    /// A display frame failed to draw. The pair stops; the next scene's
    /// present starts again.
    pub fn failed(&mut self) {
        self.stats.failed += 1;
        self.blending = false;
    }

    /// A present went out without the graph, or the timeline changed:
    /// the pair no longer follows on.
    pub fn reset(&mut self) {
        if self.newer.is_some() {
            self.stats.resets += 1;
        }
        self.older = None;
        self.newer = None;
        self.blending = false;
        self.schedule = Schedule::default();
    }

    pub fn stats(&self) -> DisplayStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Cut, SceneId};

    const TICK: Duration = Duration::from_nanos(pace::GUEST_FRAME_NS);
    const MS: Duration = Duration::from_millis(1);

    fn scene(tick: u32, published: Instant) -> Arc<FrameScene> {
        let mut s = FrameScene::default();
        s.id = SceneId { map_epoch: 1, frame: u64::from(tick), tick };
        s.stamp.ticks_run = 1;
        s.stamp.published = Some(published);
        Arc::new(s)
    }

    fn blend_alpha(at: AtSwap<u32>) -> Option<(u32, f32)> {
        match at {
            AtSwap::Blend { a, alpha } => Some((a.frame, alpha)),
            AtSwap::Newest => None,
        }
    }

    /// Runs the idle side from `from` to `to` in 1 ms steps, honouring
    /// waits, and returns what it drew: (time after `from`, plan, frames).
    fn run_idle(
        d: &mut DisplayLoop<u32>,
        from: Instant,
        to: Instant,
        rate: Rate,
    ) -> Vec<(Duration, Plan, (u32, u32))> {
        let mut out = Vec::new();
        let mut now = from;
        while now < to {
            match d.idle(now, rate) {
                Idle::Stop => break,
                Idle::Wait(at) => now = at.max(now + Duration::from_micros(1)),
                Idle::Draw { a, b, plan } => {
                    out.push((now - from, plan, (a.frame, b.frame)));
                    d.drawn(Drawn::Between(plan.into()), MS);
                    now += MS;
                }
            }
        }
        out
    }

    #[test]
    fn at_thirty_every_present_shows_its_own_scene_and_nothing_between() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        for i in 0..4u32 {
            let at = t0 + TICK * i;
            assert!(blend_alpha(d.swap(scene(i, at), i, at + MS, Rate::Off)).is_none());
            assert!(matches!(d.idle(at + 10 * MS, Rate::Off), Idle::Stop));
        }
        let s = d.stats();
        assert_eq!((s.swaps, s.blended, s.rate_hz), (4, 0, 0));
    }

    #[test]
    fn the_first_scene_has_nothing_to_blend_from() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        assert!(blend_alpha(d.swap(scene(0, t0), 0, t0 + MS, Rate::Hz(60))).is_none());
        assert!(matches!(d.idle(t0 + 20 * MS, Rate::Hz(60)), Idle::Stop));
    }

    #[test]
    fn sixty_blends_the_present_then_one_frame_between_then_the_scene_itself() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(60));
        let t1 = t0 + TICK;
        let (from, alpha) = blend_alpha(d.swap(scene(1, t1), 1, t1, Rate::Hz(60))).expect("a blend at the present");
        assert_eq!(from, 0);
        assert_eq!(alpha, 0.0, "one tick of delay: the present shows the scene before");
        d.drawn(Drawn::Swap, MS);
        let drawn = run_idle(&mut d, t1, t1 + 2 * TICK, Rate::Hz(60));
        // One blend half way, then (the next scene is late) the scene itself.
        assert_eq!(drawn.len(), 2, "{drawn:?}");
        let (at, plan, pair) = drawn[0];
        assert_eq!(pair, (0, 1));
        let Plan::Blend(a) = plan else { panic!("{plan:?}") };
        assert!((a - 0.5).abs() < 0.02, "{a}");
        assert!(at.abs_diff(Duration::from_nanos(1_000_000_000 / 60)) < MS, "{at:?}");
        assert_eq!(drawn[1].1, Plan::Newest, "a late scene holds at alpha 1, never past it");
        assert!(drawn[1].0 >= TICK, "drawn once the blend reaches 1: {:?}", drawn[1].0);
        let s = d.stats();
        assert_eq!((s.blended, s.swap_blends, s.between_newest), (2, 1, 1));
        assert_eq!(s.shown_hz, 60);
    }

    #[test]
    fn one_twenty_draws_three_blends_between_presents() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(120));
        let t1 = t0 + TICK;
        assert!(blend_alpha(d.swap(scene(1, t1), 1, t1, Rate::Hz(120))).is_some());
        // The next scene arrives on time: the frames between stop before it.
        let drawn = run_idle(&mut d, t1, t1 + TICK - MS, Rate::Hz(120));
        let alphas: Vec<f32> = drawn
            .iter()
            .map(|(_, p, _)| match p {
                Plan::Blend(a) => *a,
                Plan::Newest => 1.0,
            })
            .collect();
        assert_eq!(alphas.len(), 3, "{alphas:?}");
        for (i, a) in alphas.iter().enumerate() {
            assert!((a - (i + 1) as f32 * 0.25).abs() < 0.03, "{alphas:?}");
        }
    }

    #[test]
    fn a_cut_or_a_long_gap_shows_the_newest_scene_at_its_present() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(60));
        let t1 = t0 + 200 * MS;
        assert!(blend_alpha(d.swap(scene(1, t1), 1, t1, Rate::Hz(60))).is_none(), "a load-length gap");
        let t2 = t1 + TICK;
        let mut cut = (*scene(2, t2)).clone();
        cut.cut = Cut::LOAD;
        assert!(blend_alpha(d.swap(Arc::new(cut), 2, t2, Rate::Hz(60))).is_none(), "a cut");
        assert!(matches!(d.idle(t2 + 20 * MS, Rate::Hz(60)), Idle::Stop));
        let t3 = t2 + TICK;
        assert!(blend_alpha(d.swap(scene(3, t3), 3, t3, Rate::Hz(60))).is_some(), "blends again after");
    }

    #[test]
    fn a_reset_or_a_failure_stops_the_frames_between() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(60));
        let t1 = t0 + TICK;
        assert!(blend_alpha(d.swap(scene(1, t1), 1, t1, Rate::Hz(60))).is_some());
        d.failed();
        assert!(matches!(d.idle(t1 + 20 * MS, Rate::Hz(60)), Idle::Stop));
        d.reset();
        let t2 = t1 + TICK;
        assert!(blend_alpha(d.swap(scene(2, t2), 2, t2, Rate::Hz(60))).is_none(), "nothing before it now");
        let s = d.stats();
        assert_eq!((s.failed, s.resets), (1, 1));
    }

    #[test]
    fn the_setting_turning_to_thirty_stops_the_frames_between() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(60));
        let t1 = t0 + TICK;
        assert!(blend_alpha(d.swap(scene(1, t1), 1, t1, Rate::Hz(60))).is_some());
        assert!(matches!(d.idle(t1 + 20 * MS, Rate::Off), Idle::Stop));
    }

    #[test]
    fn display_frames_that_do_not_fit_hold_one_twenty_at_sixty() {
        let t0 = Instant::now();
        let mut d = DisplayLoop::new(t0);
        for _ in 0..32 {
            d.drawn(Drawn::Between(PlanKind::Blend), 6 * MS);
        }
        let _ = d.swap(scene(0, t0), 0, t0, Rate::Hz(120));
        let s = d.stats();
        assert!(s.capped);
        assert_eq!((s.rate_hz, s.shown_hz), (120, 60));
        assert!(s.avg_cost_us >= 5_000, "{}", s.avg_cost_us);
    }
}
