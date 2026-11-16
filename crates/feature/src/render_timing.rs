//! `render-timing`: how much of the game thread's guest time is the game's
//! render front-end, and which stage of it (docs/feature-engine.md, "7.
//! Rasterizer front-end", and docs/frame-budget.md).
//!
//! Read-only observers before and after `render_frame_and_present` and its
//! main sub-stages. The outer pair opens and closes the frame stats' guest
//! scope [`Phase::Render`], so with `H2_FRAME_STATS=1` every stats line
//! splits guest time into `cpu` (everything else) and `render`. Inside it,
//! a stack of the stages gives each its *exclusive* time: the render time
//! between entering it and leaving it, minus the stages it called. The
//! stages add up to `render` exactly, because both are read from the same
//! clock ([`timeline::phase::thread_ns`] of `Render`): other guest threads,
//! the scheduler, push-buffer decode and render-wait are not in either.
//!
//! Off by default (`H2_FEATURES=render-timing`): each hooked function
//! leaves the direct-call path, and the per-light pass runs once per
//! dynamic light. With frame stats off it measures nothing.

use std::sync::{Arc, Mutex};

use engine::points::{self, Point};
use timeline::phase::{self, Phase};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "render-timing";

/// A timed stage, outermost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// `render_frame_and_present` itself: the fallback screen, present.
    Frame = 0,
    /// `render_game_views`: view records and cameras.
    GameViews = 1,
    /// `render_views`: begin and end of the frame's drawing, the view loop.
    Views = 2,
    /// `render_game_view`: one view's setup and UI composition.
    GameView = 3,
    /// `render_scene_passes`: the pass sequence, except dynamic lights.
    ScenePasses = 4,
    /// `render_light_passes`: one dynamic light.
    LightPasses = 5,
}

pub const STAGES: usize = 6;

impl Stage {
    pub const ALL: [Stage; STAGES] = [
        Stage::Frame,
        Stage::GameViews,
        Stage::Views,
        Stage::GameView,
        Stage::ScenePasses,
        Stage::LightPasses,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Frame => "frame",
            Stage::GameViews => "game_views",
            Stage::Views => "views",
            Stage::GameView => "game_view",
            Stage::ScenePasses => "scene_passes",
            Stage::LightPasses => "light_passes",
        }
    }

    pub fn point(self) -> &'static Point {
        match self {
            Stage::Frame => &points::RENDER_FRAME,
            Stage::GameViews => &points::RENDER_GAME_VIEWS,
            Stage::Views => &points::RENDER_VIEWS,
            Stage::GameView => &points::RENDER_GAME_VIEW,
            Stage::ScenePasses => &points::RENDER_SCENE_PASSES,
            Stage::LightPasses => &points::RENDER_LIGHT_PASSES,
        }
    }
}

/// Time and calls per stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Outermost frames left (`render_frame_and_present` returned).
    pub frames: u64,
    /// Exclusive nanoseconds per [`Stage`].
    pub ns: [u64; STAGES],
    pub calls: [u64; STAGES],
}

impl Totals {
    /// All stages: the render front-end's time.
    pub fn render_ns(&self) -> u64 {
        self.ns.iter().sum()
    }

    fn add(&mut self, o: &Totals) {
        self.frames += o.frames;
        for (a, b) in self.ns.iter_mut().zip(o.ns) {
            *a += b;
        }
        for (a, b) in self.calls.iter_mut().zip(o.calls) {
            *a += b;
        }
    }

    /// The log line: ms and calls per frame.
    pub fn line(&self) -> String {
        let n = self.frames.max(1) as f64;
        let ms = |ns: u64| ns as f64 / 1e6 / n;
        let mut s = format!(
            "{} frames; ms/frame render {:.2} =",
            self.frames,
            ms(self.render_ns())
        );
        for (i, st) in Stage::ALL.iter().enumerate() {
            let plus = if i == 0 { "" } else { " +" };
            s += &format!("{plus} {} {:.2}", st.label(), ms(self.ns[*st as usize]));
        }
        s += "; calls/frame";
        for st in &Stage::ALL[1..] {
            s += &format!(" {} {:.1}", st.label(), self.calls[*st as usize] as f64 / n);
        }
        s
    }
}

/// Exclusive time per stage from a clock that only moves while the render
/// front-end runs. Pure: the tests drive the clock.
#[derive(Debug, Default)]
pub struct StageClock {
    stack: Vec<Stage>,
    mark: u64,
    /// Since the last [`take`](Self::take).
    acc: Totals,
}

impl StageClock {
    fn charge_top(&mut self, now: u64) {
        if let Some(&top) = self.stack.last() {
            self.acc.ns[top as usize] += now.saturating_sub(self.mark);
        }
        self.mark = now;
    }

    pub fn enter(&mut self, stage: Stage, now: u64) {
        self.charge_top(now);
        self.stack.push(stage);
        self.acc.calls[stage as usize] += 1;
    }

    /// Leaves `stage`. A stage left without being entered is ignored; one
    /// whose inner stages never returned (a guest fault unwound them) takes
    /// them off the stack too. Returns true when the stack is now empty.
    pub fn leave(&mut self, stage: Stage, now: u64) -> bool {
        if !self.stack.contains(&stage) {
            return self.stack.is_empty();
        }
        self.charge_top(now);
        while let Some(s) = self.stack.pop() {
            if s == stage {
                break;
            }
        }
        if stage == Stage::Frame || self.stack.is_empty() {
            self.acc.frames += 1;
        }
        self.stack.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// What was counted since the last take.
    pub fn take(&mut self) -> Totals {
        std::mem::take(&mut self.acc)
    }
}

type Clock = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

struct State {
    stages: StageClock,
    /// Since the last log line.
    window: Totals,
    /// Since install.
    run: Totals,
    last_log_ns: u64,
    warned_off: bool,
}

/// The feature. See the module docs.
pub struct RenderTiming {
    clock: Clock,
    /// Log a line this often (wall time); `None` for tests.
    log_every_ns: Option<u64>,
    state: Arc<Mutex<State>>,
}

impl Default for RenderTiming {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderTiming {
    /// Reads the frame stats' render clock and logs a line a second.
    pub fn new() -> Self {
        let mut t = Self::with_clock(Arc::new(|| phase::thread_ns(Phase::Render)));
        t.log_every_ns = Some(1_000_000_000);
        t
    }

    /// Reads `clock` instead (`None`: not measuring) and never logs.
    pub fn with_clock(clock: Clock) -> Self {
        RenderTiming {
            clock,
            log_every_ns: None,
            state: Arc::new(Mutex::new(State {
                stages: StageClock::default(),
                window: Totals::default(),
                run: Totals::default(),
                last_log_ns: 0,
                warned_off: false,
            })),
        }
    }

    /// Everything counted since install.
    pub fn totals(&self) -> Totals {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let t = s.stages.take();
        s.window.add(&t);
        s.run.add(&t);
        s.run
    }
}

impl Feature for RenderTiming {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Render front-end timing")
            .off_by_default()
            .doc("docs/frame-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        for stage in Stage::ALL {
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            // The frame's clock reads sit outside the scope: the render
            // clock stands still there, so the stages add up to the phase.
            let enter = move |_: &crate::Obs| {
                let now = clock();
                if stage == Stage::Frame {
                    phase::scope_begin(Phase::Render);
                }
                let Some(now) = now else {
                    return;
                };
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.stages.enter(stage, now);
            };
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            let log_every = self.log_every_ns;
            let leave = move |_: &crate::Obs| {
                if stage == Stage::Frame {
                    phase::scope_end();
                }
                let now = clock();
                {
                    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                    match now {
                        Some(now) => {
                            if s.stages.leave(stage, now) {
                                let t = s.stages.take();
                                s.window.add(&t);
                                s.run.add(&t);
                                log(&mut s, log_every);
                            }
                        }
                        None if stage == Stage::Frame && log_every.is_some() && !s.warned_off => {
                            s.warned_off = true;
                            eprintln!(
                                "render-timing: frame stats are off, nothing is measured (set H2_FRAME_STATS=1)"
                            );
                        }
                        None => {}
                    }
                }
            };
            let raw = b.raw();
            let h = raw.observe_before(stage.point(), stage.label(), enter);
            if stage != Stage::Frame {
                h.optional();
            }
            let raw = b.raw();
            let h = raw.observe_after(stage.point(), stage.label(), leave);
            if stage != Stage::Frame {
                h.optional();
            }
        }
        Ok(())
    }
}

fn log(s: &mut State, every: Option<u64>) {
    let Some(every) = every else {
        return;
    };
    let now = timeline::now_ns();
    if s.last_log_ns == 0 {
        s.last_log_ns = now;
        return;
    }
    if now.saturating_sub(s.last_log_ns) < every {
        return;
    }
    s.last_log_ns = now;
    let w = std::mem::take(&mut s.window);
    eprintln!("render-timing: {}", w.line());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_time_per_stage_adds_up_to_the_frame() {
        let mut c = StageClock::default();
        c.enter(Stage::Frame, 100);
        c.enter(Stage::GameViews, 101); // frame 1
        c.enter(Stage::Views, 103); // game_views 2
        for v in 0..2u64 {
            let t = 110 + v * 100;
            c.enter(Stage::GameView, t); // views 7, then 10
            c.enter(Stage::ScenePasses, t + 5); // game_view 5
            for l in 0..3u64 {
                c.enter(Stage::LightPasses, t + 10 + l * 20); // scene 5, then 10, 10
                assert!(!c.leave(Stage::LightPasses, t + 20 + l * 20)); // light 10
            }
            c.leave(Stage::ScenePasses, t + 70); // scene + 10
            c.leave(Stage::GameView, t + 90); // game_view + 20
        }
        c.leave(Stage::Views, 300); // views + 0 (after 210 + 90)
        c.leave(Stage::GameViews, 301); // game_views + 1
        assert!(c.leave(Stage::Frame, 310)); // frame + 9
        let t = c.take();
        assert_eq!(t.frames, 1);
        assert_eq!(t.ns, [10, 3, 17, 50, 70, 60]);
        assert_eq!(t.render_ns(), 210, "the stages are the whole render time");
        assert_eq!(t.calls, [1, 1, 1, 2, 2, 6]);
        assert_eq!(c.take(), Totals::default());
    }

    #[test]
    fn a_stage_that_never_returned_is_unwound_by_its_caller() {
        let mut c = StageClock::default();
        c.enter(Stage::Frame, 0);
        c.enter(Stage::Views, 1);
        c.enter(Stage::GameView, 2);
        // game_view's exit never ran; views returns.
        assert!(!c.leave(Stage::Views, 10));
        assert_eq!(c.depth(), 1);
        // An exit with no entry (collection started inside it) is ignored.
        assert!(!c.leave(Stage::ScenePasses, 11));
        assert!(c.leave(Stage::Frame, 12));
        let t = c.take();
        assert_eq!(t.ns[Stage::GameView as usize], 8);
        assert_eq!(t.ns[Stage::Frame as usize], 1 + 2);
        assert_eq!(t.frames, 1);
    }

    #[test]
    fn the_line_names_every_stage() {
        let t = Totals {
            frames: 2,
            ns: [1_000_000, 0, 0, 2_000_000, 30_000_000, 7_000_000],
            calls: [2, 2, 2, 2, 2, 24],
        };
        assert_eq!(
            t.line(),
            "2 frames; ms/frame render 20.00 = frame 0.50 + game_views 0.00 + views 0.00 + game_view 1.00 + scene_passes 15.00 + light_passes 3.50; calls/frame game_views 1.0 views 1.0 game_view 1.0 scene_passes 1.0 light_passes 12.0"
        );
    }
}
