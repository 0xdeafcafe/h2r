//! `render-timing`: how much of the game thread's guest time is the game's
//! render front-end, which stage of it, and which call in the scene pass
//! sequence (docs/feature-engine.md, "7. Rasterizer front-end", and
//! docs/frame-budget.md).
//!
//! Read-only observers before and after `render_frame_and_present` and its
//! main sub-stages. The outer pair opens and closes the frame stats' guest
//! scope [`Phase::Render`], so with `H2_FRAME_STATS=1` every stats line
//! splits guest time into `cpu` (everything else), `render`, and `d3d` (our
//! D3D entries, wherever they are called from). Inside the scope a stack of
//! the stages gives each its *exclusive* time: the time between entering
//! it and leaving it, minus the stages it called, kept as two numbers,
//! guest code (`Render`) and D3D entries (`D3d`). Both come from
//! [`timeline::phase::scope_ns`], which only moves inside the scope on its
//! owner's slices, so the stages add up exactly to the phases' time inside
//! the scope.
//!
//! **Call sites.** `render_scene_passes` is one straight run of 98 calls
//! to 37 functions ([`SCENE_CALLEES`]). Each callee gets an observer before
//! it; when its return address lies inside `render_scene_passes`, the time
//! from there to the next such call (or a nested stage, or the end) is that
//! call site's. The scene passes' own code between calls is a handful of
//! instructions and goes to the call before it.
//!
//! Off by default (`H2_FEATURES=render-timing`): each hooked function
//! leaves the direct-call path. The 37 callees are observed from every
//! caller, not only the scene passes; the line reports how many calls a
//! frame that was (`foreign`), as a bound on the overhead it adds. With
//! frame stats off it measures nothing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use engine::points::{self, Point, Sig};
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

/// The render front-end's two clocks: guest code and D3D entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub guest: u64,
    pub d3d: u64,
}

impl Now {
    pub fn new(guest: u64, d3d: u64) -> Self {
        Now { guest, d3d }
    }
}

/// Exclusive time of one stage or call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    pub guest: u64,
    pub d3d: u64,
}

impl Time {
    pub fn total(&self) -> u64 {
        self.guest + self.d3d
    }
    fn add(&mut self, o: Time) {
        self.guest += o.guest;
        self.d3d += o.d3d;
    }
}

/// One call site in `render_scene_passes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub target: u32,
    pub time: Time,
    pub calls: u64,
}

/// `render_scene_passes`' extent: return addresses in here are its calls.
pub const SCENE_PASSES_END: u32 = 0x0002_c336;

/// The functions `render_scene_passes` calls (all direct calls, read with
/// `h2 disasm`; docs/render-loop.md).
pub const SCENE_CALLEES: [u32; 37] = [
    0x0001_3450,
    0x0001_3630,
    0x0001_3870,
    0x0001_4b60,
    0x0001_4bc0,
    0x0001_5370,
    0x0001_5680,
    0x0001_e370,
    0x0002_0770,
    0x0002_5d50,
    0x0002_5dc0,
    0x0002_89e0,
    0x0002_afa0,
    0x0002_c340,
    0x0002_c370,
    0x0002_dd30,
    0x0003_35e0,
    0x0003_1c80,
    0x0003_6b50,
    0x0003_ba90,
    0x0003_c3c0,
    0x0003_ede0,
    0x0003_fdf0,
    0x0004_04b0,
    0x0004_0870,
    0x0004_0e90,
    0x0004_2b20,
    0x0004_4370,
    0x0004_4550,
    0x0004_45d0,
    0x000c_3e90,
    0x000c_bd50,
    0x0014_ddc0,
    0x0014_de70,
    0x0015_5760,
    0x0015_95d0,
    0x0015_9610,
];

/// Whether `ret` (a callee's return address) is a call in
/// `render_scene_passes`.
pub fn in_scene_passes(ret: u32) -> bool {
    ret > points::RENDER_SCENE_PASSES.base && ret <= SCENE_PASSES_END
}

/// Time and calls per stage, and per scene-pass call site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Outermost frames left (`render_frame_and_present` returned).
    pub frames: u64,
    /// Exclusive time per [`Stage`].
    pub time: [Time; STAGES],
    pub calls: [u64; STAGES],
    /// By return address.
    pub sites: BTreeMap<u32, Site>,
    /// Calls to a scene callee from anywhere else.
    pub foreign: u64,
}

impl Totals {
    /// All stages: the render front-end's guest time.
    pub fn render_ns(&self) -> u64 {
        self.time.iter().map(|t| t.guest).sum::<u64>()
            + self.sites.values().map(|s| s.time.guest).sum::<u64>()
    }
    /// D3D entries called from the render front-end.
    pub fn d3d_ns(&self) -> u64 {
        self.time.iter().map(|t| t.d3d).sum::<u64>()
            + self.sites.values().map(|s| s.time.d3d).sum::<u64>()
    }
    /// A stage with its call sites folded in (`scene_passes` as a whole).
    pub fn stage_with_sites(&self, st: Stage) -> Time {
        let mut t = self.time[st as usize];
        if st == Stage::ScenePasses {
            for s in self.sites.values() {
                t.add(s.time);
            }
        }
        t
    }

    fn add(&mut self, o: &Totals) {
        self.frames += o.frames;
        for (a, b) in self.time.iter_mut().zip(o.time) {
            a.add(b);
        }
        for (a, b) in self.calls.iter_mut().zip(o.calls) {
            *a += b;
        }
        for (k, v) in &o.sites {
            let s = self.sites.entry(*k).or_default();
            s.target = v.target;
            s.time.add(v.time);
            s.calls += v.calls;
        }
        self.foreign += o.foreign;
    }

    /// The log line: ms and calls per frame, scene passes with their calls.
    pub fn line(&self) -> String {
        let n = self.frames.max(1) as f64;
        let ms = |ns: u64| ns as f64 / 1e6 / n;
        let mut s = format!(
            "{} frames; ms/frame render {:.2} d3d {:.2} =",
            self.frames,
            ms(self.render_ns()),
            ms(self.d3d_ns())
        );
        for (i, st) in Stage::ALL.iter().enumerate() {
            let plus = if i == 0 { "" } else { " +" };
            s += &format!(
                "{plus} {} {:.2}",
                st.label(),
                ms(self.stage_with_sites(*st).total())
            );
        }
        s += "; calls/frame";
        for st in &Stage::ALL[1..] {
            s += &format!(" {} {:.1}", st.label(), self.calls[*st as usize] as f64 / n);
        }
        s += &format!("; foreign {:.0}", self.foreign as f64 / n);
        s
    }

    /// The `top` scene-pass call sites by ms/frame, one per line:
    /// `site 0x0002c06a -> 0x0002c370 3.21 ms (guest 2.90 d3d 0.31) 1.0 calls`.
    pub fn top_sites(&self, top: usize) -> Vec<String> {
        let n = self.frames.max(1) as f64;
        let ms = |ns: u64| ns as f64 / 1e6 / n;
        let mut v: Vec<(&u32, &Site)> = self.sites.iter().collect();
        v.sort_by(|a, b| b.1.time.total().cmp(&a.1.time.total()).then(a.0.cmp(b.0)));
        let own = self.time[Stage::ScenePasses as usize];
        let mut out: Vec<String> = v
            .into_iter()
            .take(top)
            .map(|(site, s)| {
                format!(
                    "site {:#010x} -> {:#010x} {:.2} ms (guest {:.2} d3d {:.2}) {:.1} calls",
                    site - 5,
                    s.target,
                    ms(s.time.total()),
                    ms(s.time.guest),
                    ms(s.time.d3d),
                    s.calls as f64 / n
                )
            })
            .collect();
        out.push(format!(
            "scene_passes own code {:.2} ms (guest {:.2} d3d {:.2})",
            ms(own.total()),
            ms(own.guest),
            ms(own.d3d)
        ));
        out
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    stage: Stage,
    /// The scene-pass call being timed, by return address.
    site: Option<u32>,
}

/// Exclusive time per stage and call site from clocks that only move while
/// the render front-end runs. Pure: the tests drive the clock.
#[derive(Debug, Default)]
pub struct StageClock {
    stack: Vec<Entry>,
    mark: Now,
    /// Since the last [`take`](Self::take).
    acc: Totals,
}

impl StageClock {
    fn charge_top(&mut self, now: Now) {
        let d = Time {
            guest: now.guest.saturating_sub(self.mark.guest),
            d3d: now.d3d.saturating_sub(self.mark.d3d),
        };
        if let Some(top) = self.stack.last() {
            match top.site {
                Some(site) => self.acc.sites.entry(site).or_default().time.add(d),
                None => self.acc.time[top.stage as usize].add(d),
            }
        }
        self.mark = now;
    }

    pub fn enter(&mut self, stage: Stage, now: Now) {
        self.charge_top(now);
        self.stack.push(Entry { stage, site: None });
        self.acc.calls[stage as usize] += 1;
    }

    /// A scene callee was entered, returning to `ret`. Counted as a call
    /// site when the scene passes are running and `ret` is in them.
    pub fn call(&mut self, target: u32, ret: u32, now: Now) {
        let top = self.stack.last().map(|e| e.stage);
        if top != Some(Stage::ScenePasses) || !in_scene_passes(ret) {
            if !self.stack.is_empty() {
                self.acc.foreign += 1;
            }
            return;
        }
        self.charge_top(now);
        if let Some(e) = self.stack.last_mut() {
            e.site = Some(ret);
        }
        let s = self.acc.sites.entry(ret).or_default();
        s.target = target;
        s.calls += 1;
    }

    /// Leaves `stage`. A stage left without being entered is ignored; one
    /// whose inner stages never returned (a guest fault unwound them) takes
    /// them off the stack too. Returns true when the stack is now empty.
    pub fn leave(&mut self, stage: Stage, now: Now) -> bool {
        if !self.stack.iter().any(|e| e.stage == stage) {
            return self.stack.is_empty();
        }
        self.charge_top(now);
        while let Some(e) = self.stack.pop() {
            if e.stage == stage {
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

type Clock = Arc<dyn Fn() -> Option<Now> + Send + Sync>;

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

/// How many call sites the log lists.
pub const TOP_SITES: usize = 15;

impl RenderTiming {
    /// Reads the frame stats' scope clocks and logs once a second.
    pub fn new() -> Self {
        let mut t = Self::with_clock(Arc::new(|| {
            Some(Now::new(
                phase::scope_ns(Phase::Render)?,
                phase::scope_ns(Phase::D3d)?,
            ))
        }));
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
        s.run.clone()
    }
}

/// A point for each scene callee: the table's own where it names one.
fn callee_points() -> &'static [&'static Point] {
    static POINTS: OnceLock<Vec<&'static Point>> = OnceLock::new();
    POINTS.get_or_init(|| {
        SCENE_CALLEES
            .iter()
            .map(|&a| {
                points::by_base(a).unwrap_or_else(|| {
                    let name: &'static str =
                        Box::leak(format!("SCENE_CALL_{a:08x}").into_boxed_str());
                    Box::leak(Box::new(Point::new(
                        name,
                        a,
                        Sig::Opaque,
                        "docs/render-loop.md",
                    )))
                })
            })
            .collect()
    })
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
            // The frame's clock reads sit outside the scope, where the scope
            // clocks stand still, so the stages add up to the phases.
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
            };
            let h = b.raw().observe_before(stage.point(), stage.label(), enter);
            if stage != Stage::Frame {
                h.optional();
            }
            let h = b.raw().observe_after(stage.point(), stage.label(), leave);
            if stage != Stage::Frame {
                h.optional();
            }
        }
        for point in callee_points() {
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            let target = point.base;
            let call = move |o: &crate::Obs| {
                // Cheap rejection first: most calls are from elsewhere.
                let Ok(ret) = o.guest().u32(o.cpu.esp) else {
                    return;
                };
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                if s.stages.depth() == 0 {
                    return;
                }
                if !in_scene_passes(ret) {
                    s.stages.acc.foreign += 1;
                    return;
                }
                drop(s);
                let Some(now) = clock() else {
                    return;
                };
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.stages.call(target, ret, now);
            };
            b.raw().observe_before(point, "scene call", call).optional();
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
    for l in w.top_sites(TOP_SITES) {
        eprintln!("render-timing:   {l}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(t: u64) -> Now {
        Now::new(t, 0)
    }
    fn guest(t: &Totals) -> Vec<u64> {
        t.time.iter().map(|t| t.guest).collect()
    }

    #[test]
    fn exclusive_time_per_stage_adds_up_to_the_frame() {
        let mut c = StageClock::default();
        c.enter(Stage::Frame, g(100));
        c.enter(Stage::GameViews, g(101)); // frame 1
        c.enter(Stage::Views, g(103)); // game_views 2
        for v in 0..2u64 {
            let t = 110 + v * 100;
            c.enter(Stage::GameView, g(t)); // views 7, then 10
            c.enter(Stage::ScenePasses, g(t + 5)); // game_view 5
            for l in 0..3u64 {
                c.enter(Stage::LightPasses, g(t + 10 + l * 20)); // scene 5, then 10, 10
                assert!(!c.leave(Stage::LightPasses, g(t + 20 + l * 20))); // light 10
            }
            c.leave(Stage::ScenePasses, g(t + 70)); // scene + 10
            c.leave(Stage::GameView, g(t + 90)); // game_view + 20
        }
        c.leave(Stage::Views, g(300)); // views + 0 (after 210 + 90)
        c.leave(Stage::GameViews, g(301)); // game_views + 1
        assert!(c.leave(Stage::Frame, g(310))); // frame + 9
        let t = c.take();
        assert_eq!(t.frames, 1);
        assert_eq!(guest(&t), [10, 3, 17, 50, 70, 60]);
        assert_eq!(t.render_ns(), 210, "the stages are the whole render time");
        assert_eq!(t.calls, [1, 1, 1, 2, 2, 6]);
        assert_eq!(c.take(), Totals::default());
    }

    #[test]
    fn scene_pass_calls_split_the_scene_passes_by_site() {
        let base = points::RENDER_SCENE_PASSES.base;
        let (a, b) = (base + 0x3f, base + 0x36e); // two return addresses
        let mut c = StageClock::default();
        c.enter(Stage::Frame, Now::new(0, 0));
        c.enter(Stage::ScenePasses, Now::new(1, 0)); // frame 1
        c.call(0x0004_4370, a, Now::new(3, 0)); // scene own 2
        // Inside site a: a call to a scene callee from elsewhere.
        c.call(0x0001_5370, 0x0001_0000, Now::new(4, 1));
        c.call(0x0003_1c80, b, Now::new(10, 4)); // site a: guest 7 d3d 4
        c.enter(Stage::LightPasses, Now::new(12, 4)); // site b: guest 2
        c.leave(Stage::LightPasses, Now::new(20, 9)); // light: guest 8 d3d 5
        c.leave(Stage::ScenePasses, Now::new(21, 9)); // site b: guest 1
        assert!(c.leave(Stage::Frame, Now::new(22, 9))); // frame 1
        let t = c.take();
        assert_eq!(
            t.sites[&a],
            Site {
                target: 0x0004_4370,
                time: Time { guest: 7, d3d: 4 },
                calls: 1
            }
        );
        assert_eq!(
            t.sites[&b],
            Site {
                target: 0x0003_1c80,
                time: Time { guest: 3, d3d: 0 },
                calls: 1
            }
        );
        assert_eq!(
            t.time[Stage::ScenePasses as usize],
            Time { guest: 2, d3d: 0 }
        );
        assert_eq!(
            t.time[Stage::LightPasses as usize],
            Time { guest: 8, d3d: 5 }
        );
        assert_eq!(t.foreign, 1);
        assert_eq!(
            t.render_ns(),
            22,
            "stages and sites are the whole guest time"
        );
        assert_eq!(t.d3d_ns(), 9);
        assert_eq!(
            t.stage_with_sites(Stage::ScenePasses),
            Time { guest: 12, d3d: 4 }
        );
        let top = t.top_sites(15);
        assert_eq!(top.len(), 3);
        assert!(
            top[0].starts_with(&format!("site {:#010x} -> 0x00044370 0.00 ms", a - 5)),
            "{top:?}"
        );
        assert!(top[2].starts_with("scene_passes own code"), "{top:?}");
    }

    #[test]
    fn a_stage_that_never_returned_is_unwound_by_its_caller() {
        let mut c = StageClock::default();
        c.enter(Stage::Frame, g(0));
        c.enter(Stage::Views, g(1));
        c.enter(Stage::GameView, g(2));
        // game_view's exit never ran; views returns.
        assert!(!c.leave(Stage::Views, g(10)));
        assert_eq!(c.depth(), 1);
        // An exit with no entry (collection started inside it) is ignored.
        assert!(!c.leave(Stage::ScenePasses, g(11)));
        assert!(c.leave(Stage::Frame, g(12)));
        let t = c.take();
        assert_eq!(t.time[Stage::GameView as usize].guest, 8);
        assert_eq!(t.time[Stage::Frame as usize].guest, 1 + 2);
        assert_eq!(t.frames, 1);
    }

    #[test]
    fn the_line_names_every_stage() {
        let ms = |guest: u64, d3d: u64| Time {
            guest: guest * 500_000,
            d3d: d3d * 500_000,
        };
        let t = Totals {
            frames: 2,
            time: [ms(2, 0), ms(0, 0), ms(0, 0), ms(4, 0), ms(56, 4), ms(12, 2)],
            calls: [2, 2, 2, 2, 2, 24],
            sites: BTreeMap::new(),
            foreign: 300,
        };
        assert_eq!(
            t.line(),
            "2 frames; ms/frame render 18.50 d3d 1.50 = frame 0.50 + game_views 0.00 + views 0.00 + game_view 1.00 + scene_passes 15.00 + light_passes 3.50; calls/frame game_views 1.0 views 1.0 game_view 1.0 scene_passes 1.0 light_passes 12.0; foreign 150"
        );
    }
}
