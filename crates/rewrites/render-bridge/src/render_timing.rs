//! `render-timing`: how much of the game thread's guest time is the game's
//! render front-end, which stage of it, and which call in the scene pass
//! sequence (docs/engine/feature-engine.md, "7. Rasterizer front-end", and
//! docs/perf/notes/gameplay-budget.md).
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
//! **Shader draw.** Shader draw begin (`0x0001bbf0`) ends in the shader
//! draw `0x00016b90`, which calls 21 functions ([`SHADER_CALLEES`]): the
//! state cache's checks and its D3D state setters. Those calls are split
//! the same way, as the shader-begin stage's call sites, and every stage
//! and site also counts the D3D entries it made
//! ([`timeline::phase::scope_entries`]).
//!
//! **Hook cost.** A call to a hooked function is never a direct call
//! between generated handlers (`runtime::direct`: only native handlers are
//! direct targets). It hands the transfer up to the runner, unwinding every
//! generated frame above it, and each of those callers then resumes and
//! returns through the runner too. That cost lands in whichever stage or
//! site is open, mostly the hooked call's callers. For the frame stages and
//! the scene passes' ~100 calls a frame it is small. The per-draw stages
//! (layer draw and below, the shader draw's sites, the helpers: hundreds to
//! thousands of calls a frame) are hooked only with [`DEEP`]
//! (`H2_FEATURES=render-timing,render-timing.deep`), and there their
//! numbers rank hook overhead as much as work. For per-draw cost use the
//! sampling profiler (docs/perf/profiling.md, `H2_PERF_FROM=level H2_PROFILE=...`),
//! which does not move the code off the direct path.
//!
//! Off by default (`H2_FEATURES=render-timing`): each hooked function
//! leaves the direct-call path. The 37 callees are observed from every
//! caller, not only the scene passes; the line reports how many calls a
//! frame that was (`foreign`), as a bound on the overhead it adds. With
//! frame stats off it measures nothing.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use guest_layout::points::{self, Point, Sig};
use timeline::phase::{self, Phase};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "render-timing";

/// The switch for the per-draw stages (`H2_FEATURES=render-timing,render-timing.deep`).
pub const DEEP: &str = "render-timing.deep";

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
    /// `0x00044550`, a layer draw, except what follows.
    LayerDraw = 6,
    /// Its setup call `0x00016b10`.
    LayerSetup = 7,
    /// `0x0003bc30`'s own code and the entry handlers it calls, except
    /// what follows.
    LayerEntries = 8,
    /// `0x0003bab0`: building (and ordering) the layer's entries.
    LayerBuild = 9,
    /// Shader draw begin `0x0001bbf0`, wherever it is called.
    ShaderBegin = 10,
    /// Geometry submission `0x00041040`, wherever it is called.
    Geometry = 11,
    /// A shader parameter's evaluation `0x0001b230`, inside the render
    /// front-end only, except what follows.
    Parameter = 12,
    /// A tag function's evaluation `0x0013b390`, inside the render front-end.
    Function = 13,
    /// The CRT `fmod` `0x00373b6a`, inside the render front-end.
    Fmod = 14,
    /// A texture binding's source `0x00019530`, inside the render front-end.
    TextureSource = 15,
    /// An animated bitmap's frame `0x000143c0`, inside the render front-end.
    BitmapFrame = 16,
}

pub const STAGES: usize = 17;

impl Stage {
    pub const ALL: [Stage; STAGES] = [
        Stage::Frame,
        Stage::GameViews,
        Stage::Views,
        Stage::GameView,
        Stage::ScenePasses,
        Stage::LightPasses,
        Stage::LayerDraw,
        Stage::LayerSetup,
        Stage::LayerEntries,
        Stage::LayerBuild,
        Stage::ShaderBegin,
        Stage::Geometry,
        Stage::Parameter,
        Stage::Function,
        Stage::Fmod,
        Stage::TextureSource,
        Stage::BitmapFrame,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Frame => "frame",
            Stage::GameViews => "game_views",
            Stage::Views => "views",
            Stage::GameView => "game_view",
            Stage::ScenePasses => "scene_passes",
            Stage::LightPasses => "light_passes",
            Stage::LayerDraw => "layer_draw",
            Stage::LayerSetup => "layer_setup",
            Stage::LayerEntries => "layer_entries",
            Stage::LayerBuild => "layer_build",
            Stage::ShaderBegin => "shader_begin",
            Stage::Geometry => "geometry",
            Stage::Parameter => "parameter",
            Stage::Function => "function",
            Stage::Fmod => "fmod",
            Stage::TextureSource => "texture_source",
            Stage::BitmapFrame => "bitmap_frame",
        }
    }

    /// Helpers the rest of the game calls too: timed only when they run
    /// inside the render front-end (another stage is open).
    /// Per-draw stages, hooked only with [`DEEP`]: called hundreds of
    /// times a frame, where the hooks' own cost swamps what they measure.
    pub fn deep(self) -> bool {
        self as usize >= Stage::LayerDraw as usize
    }

    pub fn nested_only(self) -> bool {
        matches!(
            self,
            Stage::Parameter
                | Stage::Function
                | Stage::Fmod
                | Stage::TextureSource
                | Stage::BitmapFrame
        )
    }

    pub fn point(self) -> &'static Point {
        match self {
            Stage::Frame => &points::RENDER_FRAME,
            Stage::GameViews => &points::RENDER_GAME_VIEWS,
            Stage::Views => &points::RENDER_VIEWS,
            Stage::GameView => &points::RENDER_GAME_VIEW,
            Stage::ScenePasses => &points::RENDER_SCENE_PASSES,
            Stage::LightPasses => &points::RENDER_LIGHT_PASSES,
            Stage::LayerDraw => &points::LAYER_DRAW,
            Stage::LayerSetup => &points::LAYER_SETUP,
            Stage::LayerEntries => &points::LAYER_ENTRIES,
            Stage::LayerBuild => &points::LAYER_BUILD,
            Stage::ShaderBegin => &points::SHADER_DRAW_BEGIN,
            Stage::Geometry => &points::GEOMETRY_SUBMIT,
            Stage::Parameter => &points::SHADER_PARAMETER,
            Stage::Function => &points::FUNCTION_EVALUATE,
            Stage::Fmod => &points::CRT_FMOD,
            Stage::TextureSource => &points::TEXTURE_SOURCE,
            Stage::BitmapFrame => &points::BITMAP_FRAME,
        }
    }
}

/// The render front-end's clocks: guest code and D3D entries, and a count
/// of D3D entries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub guest: u64,
    pub d3d: u64,
    pub d3d_calls: u64,
}

impl Now {
    pub fn new(guest: u64, d3d: u64) -> Self {
        Now {
            guest,
            d3d,
            d3d_calls: 0,
        }
    }
    pub fn with_calls(guest: u64, d3d: u64, d3d_calls: u64) -> Self {
        Now {
            guest,
            d3d,
            d3d_calls,
        }
    }
}

/// Exclusive time of one stage or call site.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Time {
    pub guest: u64,
    pub d3d: u64,
    /// D3D entries called.
    pub d3d_calls: u64,
}

impl Time {
    pub fn total(&self) -> u64 {
        self.guest + self.d3d
    }
    fn add(&mut self, o: Time) {
        self.guest += o.guest;
        self.d3d += o.d3d;
        self.d3d_calls += o.d3d_calls;
    }
    fn since(now: Now, mark: Now) -> Time {
        Time {
            guest: now.guest.saturating_sub(mark.guest),
            d3d: now.d3d.saturating_sub(mark.d3d),
            d3d_calls: now.d3d_calls.saturating_sub(mark.d3d_calls),
        }
    }
}

/// One call site in `render_scene_passes` or the shader draw `0x00016b90`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub target: u32,
    pub time: Time,
    pub calls: u64,
}

/// `render_scene_passes`' extent: return addresses in here are its calls.
pub const SCENE_PASSES_END: u32 = 0x0002_c336;

/// The functions `render_scene_passes` calls (all direct calls, read with
/// `h2 disasm`; docs/graphics/render-loop.md).
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

/// The shader draw `0x00016b90` that shader draw begin ends in (its tail
/// call): the state cache at `0x00485b48` and its D3D state setters
/// (docs/graphics/layer-draw.md).
pub const SHADER_DRAW: u32 = 0x0001_6b90;
/// Its extent: return addresses in here are its calls.
pub const SHADER_DRAW_END: u32 = 0x0001_6f59;

/// The functions `0x00016b90` calls directly (read with `h2 disasm`).
pub const SHADER_CALLEES: [u32; 21] = [
    0x0001_6f60,
    0x0001_7000,
    0x0001_7170,
    0x0001_7420,
    0x0001_74d0,
    0x0001_8900,
    0x0001_89a0,
    0x0001_8a90,
    0x0001_8d70,
    0x0001_8e80,
    0x0001_8ee0,
    0x0001_9be0,
    0x0001_9ca0,
    0x0001_a170,
    0x0001_ab50,
    0x0001_ae70,
    0x0001_b050,
    0x0001_b0a0,
    0x0001_ba00,
    0x0001_cf50,
    0x0013_7bd0,
];

/// Whether `ret` is a call in `0x00016b90`.
pub fn in_shader_draw(ret: u32) -> bool {
    ret > SHADER_DRAW && ret <= SHADER_DRAW_END
}

/// The stage whose call sites `ret` is one of, if any.
pub fn site_stage(ret: u32) -> Option<Stage> {
    if in_scene_passes(ret) {
        Some(Stage::ScenePasses)
    } else if in_shader_draw(ret) {
        Some(Stage::ShaderBegin)
    } else {
        None
    }
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
    /// Layer draws (`0x00044550`) by layer, inclusive.
    pub layers: BTreeMap<u32, Layer>,
}

/// One layer's draws in a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layer {
    /// Everything inside its `0x00044550` calls.
    pub time: Time,
    /// `0x00044550` calls.
    pub draws: u64,
    /// Entries `0x0003bab0` built for it.
    pub entries: u64,
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
        for (ret, s) in &self.sites {
            if site_stage(*ret) == Some(st) {
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
        for (k, v) in &o.layers {
            let l = self.layers.entry(*k).or_default();
            l.time.add(v.time);
            l.draws += v.draws;
            l.entries += v.entries;
        }
    }

    /// Layers by time, one per line:
    /// `layer 1: 6.25 ms (guest 3.34 d3d 2.90) 1.0 draws 812.0 entries`.
    pub fn layer_lines(&self) -> Vec<String> {
        let n = self.frames.max(1) as f64;
        let ms = |ns: u64| ns as f64 / 1e6 / n;
        let mut v: Vec<(&u32, &Layer)> = self.layers.iter().collect();
        v.sort_by(|a, b| b.1.time.total().cmp(&a.1.time.total()).then(a.0.cmp(b.0)));
        v.into_iter()
            .map(|(k, l)| {
                format!(
                    "layer {}: {:.2} ms (guest {:.2} d3d {:.2}) {:.1} draws {:.1} entries",
                    *k as i32,
                    ms(l.time.total()),
                    ms(l.time.guest),
                    ms(l.time.d3d),
                    l.draws as f64 / n,
                    l.entries as f64 / n
                )
            })
            .collect()
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
        s += "; d3d calls/frame";
        for st in Stage::ALL {
            let c = self.stage_with_sites(st).d3d_calls;
            if c != 0 {
                s += &format!(" {} {:.1}", st.label(), c as f64 / n);
            }
        }
        s += &format!("; foreign {:.0}", self.foreign as f64 / n);
        s
    }

    /// The `top` scene-pass call sites by ms/frame, one per line:
    /// `site 0x0002c06a -> 0x0002c370 3.21 ms (guest 2.90 d3d 0.31) 1.0 calls 12.0 d3d calls`.
    pub fn top_sites(&self, top: usize) -> Vec<String> {
        self.sites_of(Stage::ScenePasses, top)
    }

    /// Every call site of the shader draw `0x00016b90`, by ms/frame, in
    /// the same form as [`top_sites`](Self::top_sites).
    pub fn shader_sites(&self) -> Vec<String> {
        self.sites_of(Stage::ShaderBegin, SHADER_CALLEES.len() + 2)
    }

    fn sites_of(&self, stage: Stage, top: usize) -> Vec<String> {
        let n = self.frames.max(1) as f64;
        let ms = |ns: u64| ns as f64 / 1e6 / n;
        let mut v: Vec<(&u32, &Site)> = self
            .sites
            .iter()
            .filter(|(ret, _)| site_stage(**ret) == Some(stage))
            .collect();
        v.sort_by(|a, b| b.1.time.total().cmp(&a.1.time.total()).then(a.0.cmp(b.0)));
        let own = self.time[stage as usize];
        let mut out: Vec<String> = v
            .into_iter()
            .take(top)
            .map(|(site, s)| {
                format!(
                    "site {:#010x} -> {:#010x} {:.2} ms (guest {:.2} d3d {:.2}) {:.1} calls {:.1} d3d calls",
                    site - 5,
                    s.target,
                    ms(s.time.total()),
                    ms(s.time.guest),
                    ms(s.time.d3d),
                    s.calls as f64 / n,
                    s.time.d3d_calls as f64 / n
                )
            })
            .collect();
        out.push(format!(
            "{} own code {:.2} ms (guest {:.2} d3d {:.2}) {:.1} d3d calls",
            stage.label(),
            ms(own.total()),
            ms(own.guest),
            ms(own.d3d),
            own.d3d_calls as f64 / n
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
    /// Open layer draws: the layer and the clocks at entry.
    layers: Vec<(u32, Now)>,
    mark: Now,
    /// Since the last [`take`](Self::take).
    acc: Totals,
}

impl StageClock {
    fn charge_top(&mut self, now: Now) {
        let d = Time::since(now, self.mark);
        if let Some(top) = self.stack.last() {
            match top.site {
                Some(site) => self.acc.sites.entry(site).or_default().time.add(d),
                None => self.acc.time[top.stage as usize].add(d),
            }
        }
        self.mark = now;
    }

    pub fn enter(&mut self, stage: Stage, now: Now) {
        if stage.nested_only() && self.stack.is_empty() {
            return;
        }
        self.charge_top(now);
        self.stack.push(Entry { stage, site: None });
        self.acc.calls[stage as usize] += 1;
    }

    /// A scene or shader-draw callee was entered, returning to `ret`.
    /// Counted as a call site when `ret` is in the scene passes or the
    /// shader draw and that stage is running.
    pub fn call(&mut self, target: u32, ret: u32, now: Now) {
        let top = self.stack.last().map(|e| e.stage);
        if top.is_none() || site_stage(ret) != top {
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
            self.layers.clear();
        }
        self.stack.is_empty()
    }

    /// A layer draw of `layer` began (after its stage was entered).
    pub fn layer_enter(&mut self, layer: u32, now: Now) {
        self.layers.push((layer, now));
    }

    /// The innermost layer draw ended.
    pub fn layer_leave(&mut self, now: Now) {
        if let Some((layer, at)) = self.layers.pop() {
            let l = self.acc.layers.entry(layer).or_default();
            l.time.add(Time::since(now, at));
            l.draws += 1;
        }
    }

    /// The innermost layer draw built `n` entries.
    pub fn entries(&mut self, n: u32) {
        let layer = self.layers.last().map_or(u32::MAX, |l| l.0);
        self.acc.layers.entry(layer).or_default().entries += n as u64;
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

/// The layer being drawn (`layer::CURRENT_LAYER`, docs/graphics/e3-lighting.md).
const CURRENT_LAYER: u32 = 0x0046_7130;

/// How many call sites the log lists.
pub const TOP_SITES: usize = 15;

/// [`TOP_SITES`], or `H2_RENDER_TIMING_SITES=<n>` (every one of the scene
/// passes' 98 with 98: a site out of the top, such as
/// `render_screen_effects`' `-> 0x00013870`).
fn top_sites_listed() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("H2_RENDER_TIMING_SITES").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(TOP_SITES)
    })
}

impl RenderTiming {
    /// Reads the frame stats' scope clocks and logs once a second.
    pub fn new() -> Self {
        let mut t = Self::with_clock(Arc::new(|| {
            Some(Now::with_calls(
                phase::scope_ns(Phase::Render)?,
                phase::scope_ns(Phase::D3d)?,
                phase::scope_entries(Phase::D3d)?,
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

    /// Why a machine snapshot must wait (docs/runtime/snapshots.md): a
    /// stage is open, between its before and after observers, so the stage
    /// stack (and, for the frame, the frame stats' render scope) is host
    /// state a load could not match. Between frames there is nothing but
    /// counts, which a load only leaves discontinuous.
    pub fn snapshot_live(&self) -> Option<String> {
        stage_live(&self.state)
    }
}

/// [`RenderTiming::snapshot_live`] over the shared state.
fn stage_live(state: &Mutex<State>) -> Option<String> {
    let depth = state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .stages
        .depth();
    (depth != 0).then(|| format!("{depth} render stages are open"))
}

/// A point for each scene and shader-draw callee: the table's own where it
/// names one.
fn callee_points() -> &'static [&'static Point] {
    static POINTS: OnceLock<Vec<&'static Point>> = OnceLock::new();
    POINTS.get_or_init(|| {
        SCENE_CALLEES
            .iter()
            .map(|&a| (a, "SCENE_CALL"))
            .chain(SHADER_CALLEES.iter().map(|&a| (a, "SHADER_CALL")))
            .map(|(a, kind)| {
                points::by_base(a).unwrap_or_else(|| {
                    let name: &'static str = Box::leak(format!("{kind}_{a:08x}").into_boxed_str());
                    Box::leak(Box::new(Point::new(
                        name,
                        a,
                        Sig::Opaque,
                        "docs/graphics/render-loop.md",
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
            .doc("docs/perf/notes/gameplay-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let deep = b.switches().feature(DEEP, false);
        // Scene calls first: at `LAYER_DRAW`, one of them, the call must be
        // attributed to its site before the layer stage opens.
        for point in callee_points() {
            if !deep && SHADER_CALLEES.contains(&point.base) {
                continue;
            }
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            let target = point.base;
            let call = move |o: &hooks::Obs| {
                // Cheap rejection first: most calls are from elsewhere.
                let Ok(ret) = o.guest().u32(o.cpu.esp) else {
                    return;
                };
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                if s.stages.depth() == 0 {
                    return;
                }
                if site_stage(ret).is_none() {
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
            let label = if SHADER_CALLEES.contains(&target) {
                "shader call"
            } else {
                "scene call"
            };
            b.raw().observe_before(point, label, call).optional();
        }
        for stage in Stage::ALL {
            if stage.deep() && !deep {
                continue;
            }
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            // The frame's clock reads sit outside the scope, where the scope
            // clocks stand still, so the stages add up to the phases.
            let enter = move |o: &hooks::Obs| {
                let now = clock();
                if stage == Stage::Frame {
                    phase::scope_begin(Phase::Render);
                }
                let Some(now) = now else {
                    return;
                };
                let layer = (stage == Stage::LayerDraw)
                    .then(|| o.guest().u32(CURRENT_LAYER).unwrap_or(u32::MAX));
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.stages.enter(stage, now);
                if let Some(layer) = layer {
                    s.stages.layer_enter(layer, now);
                }
            };
            let (clock, state) = (Arc::clone(&self.clock), Arc::clone(&self.state));
            let log_every = self.log_every_ns;
            let leave = move |o: &hooks::Obs| {
                if stage == Stage::Frame {
                    phase::scope_end();
                }
                let now = clock();
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                match now {
                    Some(now) => {
                        match stage {
                            Stage::LayerBuild => s.stages.entries(o.cpu.eax),
                            Stage::LayerDraw => s.stages.layer_leave(now),
                            _ => {}
                        }
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
        let state = Arc::clone(&self.state);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            stage_live(&state)
        }));
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
    for l in w.top_sites(top_sites_listed()) {
        eprintln!("render-timing:   {l}");
    }
    for l in w.layer_lines() {
        eprintln!("render-timing:   {l}");
    }
    for l in w.shader_sites() {
        eprintln!("render-timing:   shader {l}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm(guest: u64, d3d: u64) -> Time {
        Time {
            guest,
            d3d,
            d3d_calls: 0,
        }
    }
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
        assert_eq!(guest(&t)[..6], [10, 3, 17, 50, 70, 60]);
        assert_eq!(t.render_ns(), 210, "the stages are the whole render time");
        assert_eq!(t.calls[..6], [1, 1, 1, 2, 2, 6]);
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
                time: tm(7, 4),
                calls: 1
            }
        );
        assert_eq!(
            t.sites[&b],
            Site {
                target: 0x0003_1c80,
                time: tm(3, 0),
                calls: 1
            }
        );
        assert_eq!(t.time[Stage::ScenePasses as usize], tm(2, 0));
        assert_eq!(t.time[Stage::LightPasses as usize], tm(8, 5));
        assert_eq!(t.foreign, 1);
        assert_eq!(
            t.render_ns(),
            22,
            "stages and sites are the whole guest time"
        );
        assert_eq!(t.d3d_ns(), 9);
        assert_eq!(t.stage_with_sites(Stage::ScenePasses), tm(12, 4));
        let top = t.top_sites(15);
        assert_eq!(top.len(), 3);
        assert!(
            top[0].starts_with(&format!("site {:#010x} -> 0x00044370 0.00 ms", a - 5)),
            "{top:?}"
        );
        assert!(top[2].starts_with("scene_passes own code"), "{top:?}");
    }

    #[test]
    fn shader_draw_calls_split_shader_begin_and_count_d3d_calls() {
        let n = |guest, d3d, calls| Now::with_calls(guest, d3d, calls);
        let a = SHADER_DRAW + 0x36e; // returns from 0x0001a170 (render states)
        let b = SHADER_DRAW + 0x3a7; // and from 0x0001cf50 (SetTexture)
        let mut c = StageClock::default();
        c.enter(Stage::Frame, n(0, 0, 0));
        c.enter(Stage::ShaderBegin, n(1, 0, 0));
        c.call(0x0001_a170, a, n(3, 0, 0)); // shader_begin own 2
        // A scene callee's site does not count inside the shader draw.
        c.call(
            0x0004_4370,
            points::RENDER_SCENE_PASSES.base + 0x3f,
            n(4, 0, 0),
        );
        c.call(0x0001_cf50, b, n(9, 6, 9)); // site a: guest 6 d3d 6, 9 calls
        c.leave(Stage::ShaderBegin, n(10, 7, 10)); // site b: guest 1 d3d 1, 1 call
        assert!(c.leave(Stage::Frame, n(11, 7, 10)));
        let t = c.take();
        assert_eq!(
            t.sites[&a].time,
            Time {
                guest: 6,
                d3d: 6,
                d3d_calls: 9
            }
        );
        assert_eq!(t.sites[&b].time.d3d_calls, 1);
        assert_eq!(t.foreign, 1);
        assert_eq!(
            t.stage_with_sites(Stage::ShaderBegin),
            Time {
                guest: 9,
                d3d: 7,
                d3d_calls: 10
            }
        );
        assert_eq!(t.stage_with_sites(Stage::ScenePasses), Time::default());
        assert!(t.top_sites(15)[0].starts_with("scene_passes own code"));
        let lines = t.shader_sites();
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].starts_with(&format!("site {:#010x} -> 0x0001a170", a - 5)),
            "{lines:?}"
        );
        assert!(lines[0].ends_with("1.0 calls 9.0 d3d calls"), "{lines:?}");
        assert!(lines[2].starts_with("shader_begin own code"), "{lines:?}");
        assert!(t.line().contains("; d3d calls/frame shader_begin 10.0;"));
    }

    #[test]
    fn helpers_are_timed_only_inside_the_render_front_end() {
        let mut c = StageClock::default();
        // The simulation's fmod: no frame open, nothing counted.
        c.enter(Stage::Fmod, g(0));
        assert_eq!(c.depth(), 0);
        assert!(c.leave(Stage::Fmod, g(5)));
        c.enter(Stage::Frame, g(10));
        c.enter(Stage::ShaderBegin, g(11)); // frame 1
        c.enter(Stage::Parameter, g(12)); // shader 1
        c.enter(Stage::Fmod, g(13)); // parameter 1
        c.leave(Stage::Fmod, g(17)); // fmod 4
        c.enter(Stage::Function, g(18)); // parameter 1
        c.leave(Stage::Function, g(20)); // function 2
        c.leave(Stage::Parameter, g(21)); // parameter 1
        c.leave(Stage::ShaderBegin, g(22)); // shader 1
        assert!(c.leave(Stage::Frame, g(23))); // frame 1
        let t = c.take();
        assert_eq!(t.frames, 1);
        let time = |s: Stage| t.time[s as usize].guest;
        assert_eq!(time(Stage::Fmod), 4);
        assert_eq!(time(Stage::Function), 2);
        assert_eq!(time(Stage::Parameter), 3);
        assert_eq!(time(Stage::ShaderBegin), 2);
        assert_eq!(t.calls[Stage::Fmod as usize], 1);
        assert_eq!(t.render_ns(), 13);
    }

    #[test]
    fn a_snapshot_waits_while_a_stage_is_open() {
        let t = RenderTiming::with_clock(Arc::new(|| Some(Now::new(0, 0))));
        assert_eq!(t.snapshot_live(), None);
        t.state.lock().unwrap().stages.enter(Stage::Frame, g(0));
        t.state.lock().unwrap().stages.enter(Stage::Views, g(1));
        assert_eq!(
            t.snapshot_live().as_deref(),
            Some("2 render stages are open")
        );
        assert!(t.state.lock().unwrap().stages.leave(Stage::Frame, g(2)));
        assert_eq!(t.snapshot_live(), None);
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
    fn layers_are_timed_inclusively_with_their_entries() {
        let base = points::RENDER_SCENE_PASSES.base;
        let mut c = StageClock::default();
        c.enter(Stage::Frame, Now::new(0, 0));
        c.enter(Stage::ScenePasses, Now::new(0, 0));
        c.call(0x0004_4550, base + 0x205, Now::new(1, 0));
        c.enter(Stage::LayerDraw, Now::new(2, 0)); // site 1
        c.layer_enter(1, Now::new(2, 0));
        c.enter(Stage::LayerSetup, Now::new(3, 0)); // layer_draw 1
        c.leave(Stage::LayerSetup, Now::new(4, 1)); // setup 1 + 1
        c.enter(Stage::LayerEntries, Now::new(5, 1)); // layer_draw 1
        c.enter(Stage::LayerBuild, Now::new(6, 1)); // entries 1
        c.entries(40);
        c.leave(Stage::LayerBuild, Now::new(9, 1)); // build 3
        c.enter(Stage::ShaderBegin, Now::new(10, 1)); // entries 1
        c.leave(Stage::ShaderBegin, Now::new(11, 3)); // shader 1 + 2
        c.enter(Stage::Geometry, Now::new(12, 3)); // entries 1
        c.leave(Stage::Geometry, Now::new(14, 8)); // geometry 2 + 5
        c.leave(Stage::LayerEntries, Now::new(15, 8)); // entries 1
        c.layer_leave(Now::new(16, 8));
        c.leave(Stage::LayerDraw, Now::new(16, 8)); // layer_draw 1
        c.leave(Stage::ScenePasses, Now::new(16, 8));
        assert!(c.leave(Stage::Frame, Now::new(16, 8)));
        let t = c.take();
        let l = t.layers[&1];
        assert_eq!(l.time, tm(14, 8));
        assert_eq!((l.draws, l.entries), (1, 40));
        let time = |s: Stage| t.time[s as usize];
        assert_eq!(time(Stage::LayerDraw), tm(3, 0));
        assert_eq!(time(Stage::LayerSetup), tm(1, 1));
        assert_eq!(time(Stage::LayerEntries), tm(4, 0));
        assert_eq!(time(Stage::LayerBuild), tm(3, 0));
        assert_eq!(time(Stage::ShaderBegin), tm(1, 2));
        assert_eq!(time(Stage::Geometry), tm(2, 5));
        assert_eq!(t.sites[&(base + 0x205)].time, tm(1, 0));
        assert_eq!(t.render_ns(), 16);
        assert_eq!(
            t.layer_lines(),
            ["layer 1: 0.00 ms (guest 0.00 d3d 0.00) 1.0 draws 40.0 entries"]
        );
    }

    #[test]
    fn the_line_names_every_stage() {
        let ms = |guest: u64, d3d: u64| tm(guest * 500_000, d3d * 500_000);
        let t = Totals {
            frames: 2,
            time: [
                ms(2, 0),
                ms(0, 0),
                ms(0, 0),
                ms(4, 0),
                ms(56, 4),
                Time {
                    d3d_calls: 600,
                    ..ms(12, 2)
                },
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
                ms(0, 0),
            ],
            calls: [2, 2, 2, 2, 2, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            sites: BTreeMap::new(),
            foreign: 300,
            layers: BTreeMap::new(),
        };
        assert_eq!(
            t.line(),
            "2 frames; ms/frame render 18.50 d3d 1.50 = frame 0.50 + game_views 0.00 + views 0.00 + game_view 1.00 + scene_passes 15.00 + light_passes 3.50 + layer_draw 0.00 + layer_setup 0.00 + layer_entries 0.00 + layer_build 0.00 + shader_begin 0.00 + geometry 0.00 + parameter 0.00 + function 0.00 + fmod 0.00 + texture_source 0.00 + bitmap_frame 0.00; calls/frame game_views 1.0 views 1.0 game_view 1.0 scene_passes 1.0 light_passes 12.0 layer_draw 0.0 layer_setup 0.0 layer_entries 0.0 layer_build 0.0 shader_begin 0.0 geometry 0.0 parameter 0.0 function 0.0 fmod 0.0 texture_source 0.0 bitmap_frame 0.0; d3d calls/frame light_passes 300.0; foreign 150"
        );
    }
}
