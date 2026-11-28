//! `render.residual`'s census (docs/graphics/retire-d3d.md, "New method:
//! `render.residual`"): over a window of presents, what the guest front-end
//! still asks `d3d` to do, by the caller of each entry and by producer.
//!
//! Two inputs:
//!
//! - **Entry calls** ([`Call`]), from the adapter in `d3d`
//!   (`d3d::residual`, wired by boot's scene glue). Each says which entry
//!   ran, the return address it was called from, what the drain before it
//!   decoded (`inline`: words game code wrote before the call) and what the
//!   entry itself did (`own`). The adapter calls in only while a window is
//!   armed. They fill `totals` and the per-site rows that become `callers`.
//! - **Producer boundaries**, from observers on the guest functions in
//!   [`REGISTRY`] and on the layer draw, installed with the existing
//!   `frame-scene.census` option ([`install`]). At each boundary the device
//!   first decodes what game code wrote into the push buffer ([`Drain`]),
//!   so an inline draw is charged to the producer that was open when its
//!   words were written. Work goes to the innermost open producer only, so
//!   the producer rows sum to `totals`. Inside a render frame with no
//!   producer open it is `unknown`; outside every render frame it is
//!   `outside_render`, and so is the frame step's frame wait
//!   ([`frame_wait`]): it calls no D3D entry, and what runs while it yields
//!   is other threads' work. `unknown` is never folded into a neighbour.
//!
//! Without the option there are no producers: `unattributed` holds the
//! inline draws, which no caller row owns.
//!
//! Every hook here is placed only with `frame-scene.census`. Each acts on
//! its own context (the guest memory its `Obs` gives it, and the device
//! the render's own D3D calls use) and writes only this census, which is
//! behind a mutex the agent reads; they are marked context-only. All of
//! them have an exit observer.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use engine::memory::GuestMemory;

/// The option the producer observers ride (shared with
/// `render_bridge::entry_census`). No option of its own.
pub const OPTION: &str = "frame-scene.census";

/// Longest window, in presents.
pub const MAX_FRAMES: u32 = 3600;

/// Return addresses kept per window; later ones count in the totals only
/// (`sites_full`).
pub const MAX_SITES: usize = 4096;

/// Open producers kept at most. A deeper stack means exits were missed (a
/// fault skipped an exit observer): it is dropped and counted.
const MAX_DEPTH: usize = 16;

/// Running totals of one device, as `d3d::residual::Tally` has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Guest draws resolved.
    pub draws: u64,
    /// Of those, draws a native replacement committed.
    pub claimed: u64,
    /// Draws that reached the backend.
    pub residual: u64,
    pub push_words: u64,
    pub state_words: u64,
    pub copies: u64,
    pub uploads: u64,
    pub native_guest_binds: u64,
    pub guest_bind_refused_destroy: u64,
    pub native_rect_clears: u64,
    pub clear_refused_rect_count: u64,
}

/// One guest call of a D3D entry, as the adapter files it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub entry: &'static str,
    pub ret: u32,
    /// What the drain before the entry did.
    pub inline: Tally,
    /// What the entry did.
    pub own: Tally,
    pub ns: u64,
    /// False when the drain stopped at a push-buffer callback; the entry
    /// runs (and is filed) again after it.
    pub ran: bool,
    /// The process's `Swap` count after the call.
    pub present: u64,
}

/// An entry by what it does, for the totals' columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clear,
    Bind,
    Lock,
    Query,
    Present,
    Other,
}

impl Kind {
    /// By the entry's name (`d3d::abi::entries`).
    pub fn of(entry: &str) -> Kind {
        match entry {
            "D3DDevice_Clear" => Kind::Clear,
            "D3DDevice_SetTexture" | "D3DDevice_SetPalette" => Kind::Bind,
            e if e.starts_with("D3DDevice_SetRenderTarget") => Kind::Bind,
            "D3D_BlockOnResource" | "D3D_BlockOnTime" => Kind::Lock,
            e if e.starts_with("D3DResource_IsBusy") => Kind::Lock,
            "D3DDevice_EndVisibilityTest" | "D3DDevice_GetVisibilityTestResult" => Kind::Query,
            "D3DDevice_Swap" => Kind::Present,
            _ => Kind::Other,
        }
    }
}

/// What a window, a producer or a caller did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    /// D3D entry calls.
    pub entries: u64,
    /// Guest draws resolved, inline ones included.
    pub draws: u64,
    /// Of those, decoded from the push buffer (an inline draw).
    pub inline_draws: u64,
    pub claimed: u64,
    pub residual: u64,
    pub push_words: u64,
    pub state_words: u64,
    pub clears: u64,
    pub binds: u64,
    pub copies: u64,
    pub uploads: u64,
    pub native_guest_binds: u64,
    pub guest_bind_refused_destroy: u64,
    pub native_rect_clears: u64,
    pub clear_refused_rect_count: u64,
    pub locks: u64,
    pub queries: u64,
    pub presents: u64,
    /// Nanoseconds in the adapter.
    pub d3d_ns: u64,
}

impl Work {
    fn tally(&mut self, t: Tally) {
        self.draws += t.draws;
        self.claimed += t.claimed;
        self.residual += t.residual;
        self.push_words += t.push_words;
        self.state_words += t.state_words;
        self.copies += t.copies;
        self.uploads += t.uploads;
        self.native_guest_binds += t.native_guest_binds;
        self.guest_bind_refused_destroy += t.guest_bind_refused_destroy;
        self.native_rect_clears += t.native_rect_clears;
        self.clear_refused_rect_count += t.clear_refused_rect_count;
    }

    /// A drain outside every entry (a producer boundary's).
    fn drained(&mut self, t: Tally) {
        self.tally(t);
        self.inline_draws += t.draws;
    }

    fn call(&mut self, c: &Call) {
        self.drained(c.inline);
        self.d3d_ns += c.ns;
        if !c.ran {
            return;
        }
        self.tally(c.own);
        self.entries += 1;
        match Kind::of(c.entry) {
            Kind::Clear => self.clears += 1,
            Kind::Bind => self.binds += 1,
            Kind::Lock => self.locks += 1,
            Kind::Query => self.queries += 1,
            Kind::Present => self.presents += 1,
            Kind::Other => {}
        }
    }
}

/// One return address's calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    /// Calls by entry name.
    pub entries: BTreeMap<&'static str, u64>,
    /// Draws the entries resolved themselves.
    pub draws: u64,
    /// Draws their drains decoded.
    pub inline_draws: u64,
    /// Draws, of both, that reached the backend.
    pub residual: u64,
    pub d3d_ns: u64,
}

/// A producer's status in the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Replaced: its guest code no longer runs.
    Native,
    /// Partly replaced: a native stage draws some of its items from the
    /// scene and their guest draws are skipped; the rest, and the
    /// producer's own guest code, still run through `d3d`
    /// ([`layer_status`]).
    Partial,
    /// The opaque block's layer draws (slice R1, [`KEPT_LAYERS`]): a call
    /// whose items are all native returns without the guest body
    /// (`frame_step::layer`); a call with an item a stage cannot draw runs
    /// the body for it, and each such item is a
    /// `scene_kept_<site><layer>_<kind>_<cause>` count
    /// (`native_layers::layer_return`). Its work here is only those items'.
    /// The present is kept the same way: a published frame returns without
    /// its body, a kept one (`publish_kept_<cause>`) runs it.
    Kept,
    /// Still drawn by the guest front-end through `d3d`.
    D3d,
    /// No registered producer was open.
    Unknown,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Native => "native",
            Status::Partial => "partial",
            Status::Kept => "kept",
            Status::D3d => "d3d",
            Status::Unknown => "unknown",
        }
    }
}

/// A registered producer: a guest function whose D3D work is filed under
/// its name while it is the innermost one open. `slot` is the carried slot
/// its work lands in (`render_bridge::carried_slots`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: &'static str,
    pub entry: u32,
    pub status: Status,
    pub slot: &'static str,
}

/// The fixed producers, by the inventory in docs/graphics/retire-d3d.md
/// ("By caller"). The layer draw is a producer per site and layer
/// ([`Producer::Layer`], its status [`layer_status`]). Every row is `d3d`
/// until the slice that replaces it changes it, in the same commit as its
/// test. Callers the inventory
/// names that have no point yet (movies, the flares,
/// the untraced `Begin` callers, the first person's own entry) are filed
/// under the producer around them or `unknown`, and the `callers` table
/// names their functions.
pub const REGISTRY: &[Row] = &[
    Row {
        name: "game_view",
        entry: 0x0002_b790,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "alternate_view",
        entry: 0x0002_c790,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "scene_passes",
        entry: 0x0002_bcd0,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "sky",
        entry: engine::points::RENDER_SKY.base,
        status: Status::Partial,
        slot: "after_lights",
    },
    Row {
        name: "scene_clear",
        entry: 0x0001_4b60,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "target_select",
        entry: 0x0001_cd30,
        status: Status::Partial,
        slot: "opaque",
    },
    // The Rust object setup answers the calls it can build; the rest run
    // the guest's body (docs/graphics/native-objects.md, "Object setup").
    Row {
        name: "object_setup",
        entry: 0x0013_3790,
        status: Status::Partial,
        slot: "opaque",
    },
    // A cut from the first frame: the scene draws the first person and a
    // part it cannot draw is a named drop (docs/graphics/native-objects.md,
    // "First person").
    Row {
        name: "first_person",
        entry: 0x0003_fdf0,
        status: Status::Native,
        slot: "transparents",
    },
    Row {
        name: "object_shadows",
        entry: 0x0003_35e0,
        status: Status::Native,
        slot: "opaque",
    },
    Row {
        name: "object_shadow",
        entry: 0x0003_2350,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "water_teardown",
        entry: 0x0003_c3c0,
        status: Status::Partial,
        slot: "after_lights",
    },
    Row {
        name: "decals",
        entry: 0x0004_4040,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "dynamic_lights",
        entry: 0x0003_1c80,
        status: Status::Native,
        slot: "after_opaque",
    },
    Row {
        name: "light_passes",
        entry: 0x0003_1170,
        status: Status::Native,
        slot: "after_opaque",
    },
    // Includes sorted layer-15 pool surfaces. Layer 6 and the teardown
    // composite run outside this dispatcher.
    Row {
        name: "transparents",
        entry: 0x0001_e370,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "screen_effects",
        entry: 0x0001_3870,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "colour_pass",
        entry: 0x0002_14f0,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "screen_effect",
        entry: 0x0002_d140,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "hud",
        entry: 0x0019_17eb,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "ui",
        entry: 0x0014_7af1,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "fade",
        entry: 0x0004_0890,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "frame_tail",
        entry: 0x0001_3b90,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "loading",
        entry: 0x0002_89e0,
        status: Status::Partial,
        slot: "outside",
    },
    // The movie's tick (docs/graphics/retire-d3d-display.md, "Movies"):
    // its decode and draw are native with a native display; the stop path
    // its service runs releases the guest's texture.
    Row {
        name: "movie",
        entry: 0x0015_6040,
        status: Status::Partial,
        slot: "outside",
    },
    // The frame's own calls around the producers above
    // (docs/graphics/retire-d3d-display.md, "Frames outside a game view"): without
    // these rows their entries were `unknown` inside the frame step and
    // `outside_render` in the frame loops outside it.
    //
    // `render_views`' begin-frame setup and its loop over the views.
    Row {
        name: "views",
        entry: 0x0002_b5d0,
        status: Status::D3d,
        slot: "opaque",
    },
    // The frame outside a game view's own setup (the target, the clear,
    // the viewport through `0x0002c8a0`), from the frame step and the map
    // load's two frame loops.
    Row {
        name: "outside_view",
        entry: 0x0022_32a0,
        status: Status::D3d,
        slot: "outside",
    },
    // A frame loop outside the frame step: its view `0x0002ca00`.
    Row {
        name: "frame_loop",
        entry: 0x0012_b6f0,
        status: Status::D3d,
        slot: "outside",
    },
    // The present: a published frame returns without its body
    // (`render_frame::publish`), so its work here is only the kept
    // frames' `Swap`.
    Row {
        name: "present",
        entry: 0x0001_3f10,
        status: Status::Kept,
        slot: "outside",
    },
];

/// The index of `name` in [`REGISTRY`].
pub fn row(name: &str) -> Option<usize> {
    REGISTRY.iter().position(|r| r.name == name)
}

/// Where a layer draw is called from: `render_bridge::entry_census`'s
/// sites, by name.
pub const LAYER_SITES: [&str; 7] = [
    "scene",
    "alternate",
    "first_person",
    "light",
    "object_shadow",
    "other",
    "sorted",
];

/// The layer draws a native stage draws items of from the scene, skipping
/// those items' guest draws, while the guest body runs for every call
/// (docs/graphics/retire-d3d.md, "By caller"), by [`LAYER_SITES`] index:
/// the first person's layer 16, the light passes' 12 and 13, and the
/// object shadow's caster and receiver layers 10 and 11. Each is
/// [`Status::Partial`]: what its stages refuse stays the guest's.
pub const PARTIAL_LAYERS: [(u8, &[u8]); 3] = [
    (2, &[16]),
    (3, &[12, 13]),
    (4, &[10, 11]),
];

/// The layer draws that return when every item is native, by
/// [`LAYER_SITES`] index: the opaque block's (the scene's layers 1, 3, 5,
/// 7, 8 and 20, the alternate view's 1, 3 and 5, the first person's 1, 3,
/// 7, 8 and 20: `frame_step::layer::LAYERS`) and R5's (the scene's 2 and
/// 17, the alternate view's and the first person's 2, the sorted
/// handlers' 18, 19 and 21: `frame_step::layer::LATE_LAYERS`; the scene's
/// water 6 and fog 16: `FOG_WATER_LAYERS`), at the sites
/// `native_layers::return_site` names. Each is [`Status::Kept`].
pub const KEPT_LAYERS: [(u8, &[u8]); 4] = [
    (0, &[1, 2, 3, 5, 6, 7, 8, 16, 17, 20]),
    (1, &[1, 2, 3, 5]),
    (2, &[1, 2, 3, 7, 8, 20]),
    (6, &[18, 19, 21]),
];

/// The status of the layer draw at site `at` ([`LAYER_SITES`]) of `layer`.
pub fn layer_status(at: u8, layer: u8) -> Status {
    let listed = |table: &[(u8, &[u8])]| table.iter().any(|&(site, layers)| site == at && layers.contains(&layer));
    if listed(&KEPT_LAYERS) {
        Status::Kept
    } else if listed(&PARTIAL_LAYERS) {
        Status::Partial
    } else {
        Status::D3d
    }
}

/// Who the work is filed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Producer {
    /// A [`REGISTRY`] row, by index.
    Fixed(usize),
    /// The layer draw, by [`LAYER_SITES`] index and layer.
    Layer { at: u8, layer: u8 },
    /// Inside a render frame, no producer open.
    Unknown,
    /// No render frame open (boot, the caches, the simulation).
    OutsideRender,
}

impl Producer {
    pub fn name(self) -> String {
        match self {
            Producer::Fixed(i) => REGISTRY.get(i).map_or("?", |r| r.name).to_owned(),
            Producer::Layer { at, layer } => {
                format!(
                    "layer.{}.{layer}",
                    LAYER_SITES.get(usize::from(at)).unwrap_or(&"other")
                )
            }
            Producer::Unknown => "unknown".into(),
            Producer::OutsideRender => "outside_render".into(),
        }
    }

    pub fn status(self) -> Status {
        match self {
            Producer::Fixed(i) => REGISTRY.get(i).map_or(Status::Unknown, |r| r.status),
            Producer::Layer { at, layer } => layer_status(at, layer),
            Producer::OutsideRender => Status::D3d,
            Producer::Unknown => Status::Unknown,
        }
    }

    pub fn slot(self) -> &'static str {
        match self {
            Producer::Fixed(i) => REGISTRY.get(i).map_or("", |r| r.slot),
            Producer::Layer { at: 0, layer: 6 | 16 } => "after_lights",
            Producer::Layer { at, .. } => match LAYER_SITES.get(usize::from(at)) {
                Some(&"light") => "after_opaque",
                Some(&"sorted") => "transparents",
                _ => "opaque",
            },
            Producer::Unknown => "",
            Producer::OutsideRender => "outside",
        }
    }
}

/// One producer's work over the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProducerWork {
    /// Times it was entered.
    pub calls: u64,
    pub work: Work,
    /// Wall time while it was the innermost producer, D3D time included.
    pub inner_ns: u64,
}

impl ProducerWork {
    /// Its exclusive guest time: innermost wall time less its D3D time.
    pub fn guest_ns(&self) -> u64 {
        self.inner_ns.saturating_sub(self.work.d3d_ns)
    }
}

/// The window a report covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window {
    /// Presents in it.
    pub frames: u32,
    /// Of those, presents whose frame entered a game view (counted by the
    /// `game_view` producer, so zero without the option).
    pub game_frames: u32,
    /// The `Swap` ordinals of its first and last present.
    pub presents: [u64; 2],
    /// The last tick that had ended at its start and at its end.
    pub ticks: [Option<u64>; 2],
    pub map: Option<String>,
    /// The run has `render-timing`.
    pub timing: bool,
}

/// A finished window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub window: Window,
    pub totals: Work,
    /// By return address.
    pub sites: BTreeMap<u32, Site>,
    pub sites_full: bool,
    /// `None` without the option's observers.
    pub producers: Option<BTreeMap<Producer, ProducerWork>>,
    /// Draws and entries filed under no producer: with observers, the
    /// `unknown` row's; without, the inline draws.
    pub unattributed: (u64, u64),
    /// Producer exits with nothing to end, and producers dropped unended.
    pub unbalanced: u64,
}

/// Where a window is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Idle,
    /// Waiting for the next present to start.
    Armed,
    /// Counting.
    Open,
    /// The last window finished; nothing is armed.
    Done,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Armed | State::Open => "armed",
            State::Done => "done",
        }
    }
}

/// What the census reads from the host at a window's ends.
#[derive(Clone, Copy)]
pub struct Host {
    /// Turns the adapter's watch on or off (`d3d::residual::arm`).
    pub arm: fn(bool),
    /// The last tick that ended.
    pub tick: fn() -> Option<u64>,
    /// The map loaded.
    pub map: fn() -> Option<String>,
}

struct Open {
    producer: Producer,
}

/// The census: the window being counted, the open producers, and the last
/// finished window.
#[derive(Default)]
pub struct Census {
    state: State,
    frames: u32,
    left: u32,
    window: Window,
    totals: Work,
    sites: BTreeMap<u32, Site>,
    sites_full: bool,
    producers: BTreeMap<Producer, ProducerWork>,
    unbalanced: u64,
    /// The producer observers are installed.
    observed: bool,
    timing: bool,
    stack: Vec<Open>,
    frame_depth: u32,
    /// Inside the frame step's frame wait ([`Census::wait`]).
    waiting: bool,
    game_view_seen: bool,
    /// When the innermost producer became innermost (timing only).
    since: Option<Instant>,
    last: Option<Report>,
}

impl Census {
    /// A census whose producers are observed (`observed`) and timed
    /// (`timing`, with `render-timing`).
    pub fn new(observed: bool, timing: bool) -> Census {
        Census {
            observed,
            timing,
            ..Census::default()
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Whether a window is armed or open: boundaries drain only then.
    pub fn active(&self) -> bool {
        matches!(self.state, State::Armed | State::Open)
    }

    /// Starts a window of `frames` presents at the next present; replaces
    /// one in progress.
    pub fn arm(&mut self, frames: u32) {
        self.state = State::Armed;
        self.frames = frames.clamp(1, MAX_FRAMES);
        self.left = self.frames;
        self.reset_window();
    }

    fn reset_window(&mut self) {
        self.window = Window::default();
        self.totals = Work::default();
        self.sites.clear();
        self.sites_full = false;
        self.producers.clear();
        self.unbalanced = 0;
    }

    /// Where work goes now.
    pub fn current(&self) -> Producer {
        match self.stack.last() {
            Some(o) => o.producer,
            None if self.frame_depth > 0 && !self.waiting => Producer::Unknown,
            None => Producer::OutsideRender,
        }
    }

    fn producer(&mut self) -> &mut ProducerWork {
        let p = self.current();
        self.producers.entry(p).or_default()
    }

    /// Charges the wall time since the last boundary to the innermost
    /// producer.
    fn charge_time(&mut self) {
        if !self.timing || self.state != State::Open || !self.observed {
            self.since = None;
            return;
        }
        let now = Instant::now();
        if let Some(since) = self.since.replace(now) {
            let ns = u64::try_from(now.duration_since(since).as_nanos()).unwrap_or(u64::MAX);
            self.producer().inner_ns += ns;
        }
    }

    /// An entry call from the adapter.
    pub fn call(&mut self, c: &Call, host: Option<&Host>) {
        match self.state {
            State::Armed if c.ran && Kind::of(c.entry) == Kind::Present => {
                self.open(c.present, host)
            }
            State::Open => {
                self.totals.call(c);
                if self.observed {
                    self.producer().work.call(c);
                }
                self.site(c);
                if c.ran && Kind::of(c.entry) == Kind::Present {
                    self.presented(c.present, host);
                }
            }
            _ => {}
        }
    }

    fn site(&mut self, c: &Call) {
        if !self.sites.contains_key(&c.ret) && self.sites.len() >= MAX_SITES {
            self.sites_full = true;
            return;
        }
        let s = self.sites.entry(c.ret).or_default();
        s.inline_draws += c.inline.draws;
        s.residual += c.inline.residual;
        s.d3d_ns += c.ns;
        if c.ran {
            *s.entries.entry(c.entry).or_default() += 1;
            s.draws += c.own.draws;
            s.residual += c.own.residual;
        }
    }

    fn open(&mut self, present: u64, host: Option<&Host>) {
        self.state = State::Open;
        self.reset_window();
        self.window.frames = self.frames;
        self.window.presents = [present + 1, present + u64::from(self.frames)];
        self.window.timing = self.timing;
        self.window.ticks[0] = host.and_then(|h| (h.tick)());
        self.window.map = host.and_then(|h| (h.map)());
        self.game_view_seen = false;
        self.since = None;
        self.charge_time();
    }

    fn presented(&mut self, present: u64, host: Option<&Host>) {
        self.window.game_frames += u32::from(std::mem::take(&mut self.game_view_seen));
        self.left = self.left.saturating_sub(1);
        if self.left > 0 {
            return;
        }
        self.charge_time();
        self.window.presents[1] = present;
        self.window.ticks[1] = host.and_then(|h| (h.tick)());
        let unattributed = if self.observed {
            let u = self
                .producers
                .get(&Producer::Unknown)
                .map(|p| p.work)
                .unwrap_or_default();
            (u.draws, u.entries)
        } else {
            (self.totals.inline_draws, 0)
        };
        self.last = Some(Report {
            window: std::mem::take(&mut self.window),
            totals: std::mem::take(&mut self.totals),
            sites: std::mem::take(&mut self.sites),
            sites_full: self.sites_full,
            producers: self.observed.then(|| std::mem::take(&mut self.producers)),
            unattributed,
            unbalanced: self.unbalanced,
        });
        self.state = State::Done;
        if let Some(h) = host {
            (h.arm)(false);
        }
    }

    /// A drain at a producer boundary, before the boundary moves: charged
    /// to the producer open while its words were written.
    pub fn drained(&mut self, t: Tally) {
        if self.state != State::Open || t == Tally::default() {
            return;
        }
        self.totals.drained(t);
        if self.observed {
            self.producer().work.drained(t);
        }
    }

    /// A producer begins.
    pub fn enter(&mut self, p: Producer) {
        self.charge_time();
        if self.stack.len() >= MAX_DEPTH {
            self.unbalanced += self.stack.len() as u64;
            self.stack.clear();
        }
        if p == Producer::Fixed(GAME_VIEW) {
            self.game_view_seen = true;
        }
        self.stack.push(Open { producer: p });
        if self.state == State::Open {
            self.producer().calls += 1;
        }
    }

    /// The innermost open producer `matches` ends. Producers opened inside
    /// it that never ended are dropped and counted; an exit with nothing
    /// of its kind open is counted and ignored.
    pub fn leave(&mut self, matches: impl Fn(Producer) -> bool) {
        self.charge_time();
        let Some(i) = self.stack.iter().rposition(|o| matches(o.producer)) else {
            self.unbalanced += 1;
            return;
        };
        self.unbalanced += (self.stack.len() - 1 - i) as u64;
        self.stack.truncate(i);
    }

    /// Ends `p` if it is open; nothing is counted when it is not
    /// ([`passed_over`]).
    pub fn close_if_open(&mut self, p: Producer) {
        if self.stack.iter().any(|o| o.producer == p) {
            self.leave(|q| q == p);
        }
    }

    /// A render frame begins (`render_frame_and_present`).
    pub fn frame_begin(&mut self) {
        self.charge_time();
        self.frame_depth += 1;
    }

    /// The frame step's frame wait begins (`on`) or returns. While it
    /// runs, work with no producer open is `outside_render`: the wait
    /// calls no D3D entry, and the threads that run while it yields are
    /// not the render.
    pub fn wait(&mut self, on: bool) {
        self.charge_time();
        self.waiting = on;
    }

    /// A render frame ends. Producers still open inside it never ended.
    pub fn frame_end(&mut self) {
        self.charge_time();
        self.waiting = false;
        self.frame_depth = self.frame_depth.saturating_sub(1);
        if self.frame_depth == 0 && !self.stack.is_empty() {
            self.unbalanced += self.stack.len() as u64;
            self.stack.clear();
        }
    }

    /// A snapshot load: nothing open survives it.
    pub fn forget(&mut self) {
        self.unbalanced += self.stack.len() as u64;
        self.stack.clear();
        self.frame_depth = 0;
        self.waiting = false;
        self.since = None;
    }

    /// The last finished window.
    pub fn last(&self) -> Option<&Report> {
        self.last.as_ref()
    }

    /// Whether producers are observed.
    pub fn observed(&self) -> bool {
        self.observed
    }

    /// The innermost open producer's name, when it is a [`REGISTRY`] row.
    pub fn around(&self) -> Option<&'static str> {
        match self.stack.last()?.producer {
            Producer::Fixed(i) => REGISTRY.get(i).map(|r| r.name),
            _ => None,
        }
    }
}

/// [`REGISTRY`]'s `game_view` row.
const GAME_VIEW: usize = 0;
const _: () = assert!(REGISTRY[GAME_VIEW].entry == 0x0002_b790);

static CENSUS: Mutex<Option<Census>> = Mutex::new(None);
static HOST: Mutex<Option<Host>> = Mutex::new(None);
/// [`Census::active`], readable without the lock.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static DRAIN: Mutex<Option<Arc<dyn Drain>>> = Mutex::new(None);

/// The device, run up to here: what game code wrote into the push buffer
/// is decoded, and what that did is returned.
pub trait Drain: Send + Sync {
    fn drain(&self, mem: &mut dyn GuestMemory) -> Tally;
}

/// Registers the device's [`Drain`] (the host's D3D install).
pub fn set_drain(drain: Arc<dyn Drain>) {
    *DRAIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(drain);
}

/// Registers the host's hooks (boot's scene glue).
pub fn set_host(host: Host) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Host> {
    *HOST.lock().unwrap_or_else(|e| e.into_inner())
}

fn with<R>(f: impl FnOnce(&mut Census) -> R) -> R {
    let mut g = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    let c = g.get_or_insert_with(Census::default);
    let r = f(c);
    ACTIVE.store(c.active(), Ordering::Relaxed);
    r
}

/// An entry call from the adapter (boot's scene glue forwards
/// `d3d::residual`'s). Called with the device locked; takes only the
/// census's lock.
pub fn call(c: &Call) {
    let h = host();
    with(|census| census.call(c, h.as_ref()));
}

/// Arms a window of `frames` presents at the next present, or with `None`
/// reads; answers the state and the last finished window.
pub fn request(frames: Option<u32>) -> (State, Option<Report>) {
    let h = host();
    with(|c| {
        if let Some(n) = frames {
            c.arm(n);
            if let Some(h) = &h {
                (h.arm)(true);
            }
        }
        (c.state(), c.last().cloned())
    })
}

/// A snapshot load (the frame step's `forget`).
pub fn forget() {
    with(Census::forget);
}

/// The frame step calls its frame wait `0x0012b2e0` (`on`) or the wait
/// returned (docs/graphics/retire-d3d-display.md, "The frame step"). What
/// game code wrote before it is drained to the producer that wrote it.
pub fn frame_wait(mem: &mut dyn GuestMemory, on: bool) {
    #[cfg(feature = "native-render")]
    boundary(mem);
    #[cfg(not(feature = "native-render"))]
    let _ = mem;
    with(|c| c.wait(on));
}

/// The device drained at a boundary, when a window is armed or open.
#[cfg(feature = "native-render")]
fn boundary(mem: &mut dyn GuestMemory) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let drain = DRAIN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(d) = drain {
        let t = d.drain(mem);
        with(|c| c.drained(t));
    }
}

/// The producer at `entry` ends without the guest running it: a `before`
/// hook passed over its call (the sky's cut, `native_sky::before_sky`),
/// so its end observer does not run. Closes it when its entry observer
/// opened it; otherwise nothing is open to close and nothing is counted.
#[cfg(feature = "native-render")]
pub fn passed_over(mem: &mut dyn GuestMemory, entry: u32) {
    let Some(i) = REGISTRY.iter().position(|r| r.entry == entry) else { return };
    let p = Producer::Fixed(i);
    boundary(mem);
    with(|c| c.close_if_open(p));
}

/// Installs the producer observers (the frame step's build, with
/// [`OPTION`] set) and marks the census observed.
#[cfg(feature = "native-render")]
pub fn install(b: &mut hooks::Builder) {
    use engine::points;
    use native_render::skip::CURRENT_LAYER;
    use native_render::world_layers::Site;

    let timing = b.switches().feature(crate::render_timing::ID, false);
    {
        let mut g = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
        let last = g.as_mut().and_then(|c| c.last.take());
        *g = Some(Census {
            last,
            ..Census::new(true, timing)
        });
    }
    b.raw()
        .observe_before(&points::RENDER_FRAME, "residual_census_frame", |o| {
            boundary(&mut *o.guest());
            with(Census::frame_begin);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::RENDER_FRAME, "residual_census_frame_end", |o| {
            boundary(&mut *o.guest());
            with(Census::frame_end);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_before(&points::LAYER_DRAW, "residual_census_layer", |o| {
            let mut g = o.guest();
            let ret = g.u32(o.cpu.esp).unwrap_or(0);
            let layer = g.u32(CURRENT_LAYER).unwrap_or(u32::MAX);
            boundary(&mut *g);
            drop(g);
            let layer = u8::try_from(layer).unwrap_or(u8::MAX);
            let site = crate::entry_census::late_site(u32::from(layer), ret);
            with(|c| {
                let at = match site {
                    Site::Scene => 0,
                    Site::AlternateView => 1,
                    Site::FirstPerson => 2,
                    Site::Sky | Site::Other if native_render::world_layers::sorted_site(u32::from(layer), ret).is_some() => 6,
                    Site::Sky | Site::Other => match c.around() {
                        Some("light_passes") => 3,
                        Some("object_shadow") => 4,
                        _ => 5,
                    },
                };
                c.enter(Producer::Layer { at, layer });
            });
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::LAYER_DRAW, "residual_census_layer_end", |o| {
            boundary(&mut *o.guest());
            with(|c| c.leave(|p| matches!(p, Producer::Layer { .. })));
        })
        .context_only()
        .optional();
    for (point, name, label, end) in [
        (
            &points::RENDER_GAME_VIEW,
            "game_view",
            "residual_census_game_view",
            "residual_census_game_view_end",
        ),
        (
            &points::RENDER_ALTERNATE_VIEW,
            "alternate_view",
            "residual_census_alternate_view",
            "residual_census_alternate_view_end",
        ),
        (
            &points::RENDER_SCENE_PASSES,
            "scene_passes",
            "residual_census_scene_passes",
            "residual_census_scene_passes_end",
        ),
        (
            &points::WATER_TEARDOWN,
            "water_teardown",
            "residual_census_water_teardown",
            "residual_census_water_teardown_end",
        ),
        (
            &points::RENDER_SKY,
            "sky",
            "residual_census_sky",
            "residual_census_sky_end",
        ),
        (
            &points::SCENE_CLEAR,
            "scene_clear",
            "residual_census_scene_clear",
            "residual_census_scene_clear_end",
        ),
        (
            &points::SCENE_TARGET_SELECT,
            "target_select",
            "residual_census_target_select",
            "residual_census_target_select_end",
        ),
        (
            &points::OBJECT_SHADOW_LIST,
            "object_shadows",
            "residual_census_object_shadows",
            "residual_census_object_shadows_end",
        ),
        (
            &points::OBJECT_SHADOW,
            "object_shadow",
            "residual_census_object_shadow",
            "residual_census_object_shadow_end",
        ),
        (
            &points::DECAL_RENDER,
            "decals",
            "residual_census_decals",
            "residual_census_decals_end",
        ),
        (
            &points::RENDER_DYNAMIC_LIGHTS,
            "dynamic_lights",
            "residual_census_dynamic_lights",
            "residual_census_dynamic_lights_end",
        ),
        (
            &points::RENDER_LIGHT_PASSES,
            "light_passes",
            "residual_census_light_passes",
            "residual_census_light_passes_end",
        ),
        (
            &points::SCENE_TRANSPARENTS,
            "transparents",
            "residual_census_transparents",
            "residual_census_transparents_end",
        ),
        (
            &points::RENDER_SCREEN_EFFECTS,
            "screen_effects",
            "residual_census_screen_effects",
            "residual_census_screen_effects_end",
        ),
        (
            &points::COLOUR_PASS,
            "colour_pass",
            "residual_census_colour_pass",
            "residual_census_colour_pass_end",
        ),
        (
            &points::SCREEN_EFFECT,
            "screen_effect",
            "residual_census_screen_effect",
            "residual_census_screen_effect_end",
        ),
        (
            &points::UI_HUD_DRAW,
            "hud",
            "residual_census_hud",
            "residual_census_hud_end",
        ),
        (
            &points::UI_RENDER_LAYERS,
            "ui",
            "residual_census_ui",
            "residual_census_ui_end",
        ),
        (
            &points::UI_FADE_DRAW,
            "fade",
            "residual_census_fade",
            "residual_census_fade_end",
        ),
        (
            &points::RENDER_FRAME_TAIL,
            "frame_tail",
            "residual_census_frame_tail",
            "residual_census_frame_tail_end",
        ),
        (
            &points::UI_LOADING_DRAW,
            "loading",
            "residual_census_loading",
            "residual_census_loading_end",
        ),
        (
            &points::RENDER_VIEWS,
            "views",
            "residual_census_views",
            "residual_census_views_end",
        ),
        (
            &points::RENDER_OUTSIDE_VIEW,
            "outside_view",
            "residual_census_outside_view",
            "residual_census_outside_view_end",
        ),
        (
            &points::RENDER_FRAME_LOOP,
            "frame_loop",
            "residual_census_frame_loop",
            "residual_census_frame_loop_end",
        ),
        (
            &points::RENDER_PRESENT,
            "present",
            "residual_census_present",
            "residual_census_present_end",
        ),
    ] {
        let Some(i) = row(name) else { continue };
        debug_assert_eq!(REGISTRY[i].entry, point.base, "{name}");
        let p = Producer::Fixed(i);
        b.raw()
            .observe_before(point, label, move |o| {
                boundary(&mut *o.guest());
                with(|c| c.enter(p));
            })
            .context_only()
            .optional();
        b.raw()
            .observe_after(point, end, move |o| {
                boundary(&mut *o.guest());
                with(|c| c.leave(|q| q == p));
            })
            .context_only()
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A producer passed over closes only if it was opened, leaving the
    /// census balanced either way.
    #[test]
    fn a_passed_over_producer_closes_only_when_open() {
        let sky = row("sky").unwrap();
        let mut c = Census::new(true, false);
        c.enter(Producer::Fixed(sky));
        c.close_if_open(Producer::Fixed(sky));
        assert!(c.stack.is_empty());
        assert_eq!(c.unbalanced, 0);
        c.close_if_open(Producer::Fixed(sky));
        assert_eq!(c.unbalanced, 0, "nothing open, nothing counted");
    }

    fn tally(draws: u64, claimed: u64, residual: u64, push_words: u64) -> Tally {
        Tally {
            draws,
            claimed,
            residual,
            push_words,
            ..Tally::default()
        }
    }

    fn entry(name: &'static str, ret: u32, own: Tally) -> Call {
        Call {
            entry: name,
            ret,
            inline: Tally::default(),
            own,
            ns: 10,
            ran: true,
            present: 0,
        }
    }

    fn swap(present: u64) -> Call {
        Call {
            present,
            ..entry("D3DDevice_Swap", 0x0001_3f40, Tally::default())
        }
    }

    /// A census, observed or not, with a window of `frames` open after
    /// present 7.
    fn open(observed: bool, frames: u32) -> Census {
        let mut c = Census::new(observed, false);
        c.arm(frames);
        assert_eq!(c.state(), State::Armed);
        c.call(&swap(7), None);
        assert_eq!(c.state(), State::Open);
        c
    }

    fn close(mut c: Census, present: u64) -> Report {
        c.call(&swap(present), None);
        assert_eq!(c.state(), State::Done);
        c.last().cloned().unwrap()
    }

    const DRAW: &str = "D3DDevice_DrawVertices";

    #[test]
    fn entry_calls_from_two_return_addresses_land_in_two_caller_rows() {
        let mut c = open(false, 1);
        c.call(&entry(DRAW, 0x0004_c976, tally(1, 0, 1, 12)), None);
        c.call(&entry(DRAW, 0x0004_c976, tally(1, 0, 1, 12)), None);
        c.call(
            &entry("D3DDevice_SetTexture", 0x0002_2a70, Tally::default()),
            None,
        );
        let r = close(c, 8);
        let ret: Vec<_> = r
            .sites
            .keys()
            .copied()
            .filter(|&a| a != 0x0001_3f40)
            .collect();
        assert_eq!(ret, [0x0002_2a70, 0x0004_c976]);
        let s = &r.sites[&0x0004_c976];
        assert_eq!((s.entries[DRAW], s.draws, s.residual), (2, 2, 2));
        assert_eq!(r.sites[&0x0002_2a70].entries["D3DDevice_SetTexture"], 1);
        // The window's own closing present is its last.
        assert_eq!(r.totals.entries, 4);
        assert_eq!((r.totals.binds, r.totals.presents), (1, 1));
        assert_eq!(r.window.presents, [8, 8]);
        assert!(r.producers.is_none());
    }

    #[test]
    fn an_inline_draw_written_under_one_producer_and_drained_under_the_next_is_charged_to_the_first()
     {
        let ui = Producer::Fixed(row("ui").unwrap());
        let hud = Producer::Fixed(row("hud").unwrap());
        let mut c = open(true, 1);
        c.frame_begin();
        c.enter(hud);
        // The HUD's quad is in the push buffer; the boundary drains it
        // before the slot moves.
        c.drained(tally(1, 0, 1, 20));
        c.leave(|p| p == hud);
        c.enter(ui);
        // An entry under the UI drains nothing more and draws one itself.
        c.call(&entry(DRAW, 0x0014_8000, tally(1, 0, 1, 6)), None);
        c.leave(|p| p == ui);
        c.frame_end();
        let r = close(c, 8);
        let p = r.producers.unwrap();
        assert_eq!(
            (
                p[&hud].work.draws,
                p[&hud].work.inline_draws,
                p[&hud].work.entries
            ),
            (1, 1, 0)
        );
        assert_eq!(
            (
                p[&ui].work.draws,
                p[&ui].work.inline_draws,
                p[&ui].work.entries
            ),
            (1, 0, 1)
        );
        assert_eq!((p[&hud].calls, p[&ui].calls), (1, 1));
        assert_eq!(r.totals.draws, 2);
        assert_eq!(r.unattributed, (0, 0));
    }

    #[test]
    fn a_claimed_draw_counts_once_in_claimed_and_not_in_residual() {
        let mut c = open(true, 1);
        c.frame_begin();
        c.enter(Producer::Layer { at: 0, layer: 3 });
        // Two draws resolved: one a replacement took, one the backend drew.
        c.call(
            &entry(
                "D3DDevice_DrawIndexedVertices",
                0x0004_5000,
                tally(2, 1, 1, 30),
            ),
            None,
        );
        c.leave(|p| matches!(p, Producer::Layer { .. }));
        c.frame_end();
        let r = close(c, 8);
        assert_eq!(
            (r.totals.draws, r.totals.claimed, r.totals.residual),
            (2, 1, 1)
        );
        let layer = r.producers.unwrap()[&Producer::Layer { at: 0, layer: 3 }].work;
        assert_eq!((layer.claimed, layer.residual), (1, 1));
        assert_eq!(Producer::Layer { at: 0, layer: 3 }.name(), "layer.scene.3");
        assert_eq!(
            Producer::Layer { at: 0, layer: 3 }.status(),
            Status::Kept
        );
        assert_eq!(r.sites[&0x0004_5000].residual, 1);
    }

    #[test]
    fn the_residual_total_equals_the_draws_the_backend_received() {
        // Each call's residual is the growth of the backend's draw count
        // across it (`d3d::residual::Tally`), so their sum over the window
        // is the backend's draws in it, however they split.
        let mut c = open(true, 2);
        let mut backend = 0;
        for (i, (inline, own)) in [(0, 3), (2, 0), (1, 1), (0, 0)].into_iter().enumerate() {
            backend += inline + own;
            let call = Call {
                inline: tally(inline, 0, inline, 0),
                ..entry(DRAW, 0x100 + i as u32, tally(own, 0, own, 0))
            };
            c.call(&call, None);
            if i == 1 {
                c.call(&swap(8), None);
            }
        }
        c.drained(tally(4, 0, 4, 0));
        backend += 4;
        let r = close(c, 9);
        assert_eq!(r.totals.residual, backend);
        let by_producer: u64 = r
            .producers
            .as_ref()
            .unwrap()
            .values()
            .map(|p| p.work.residual)
            .sum();
        assert_eq!(by_producer, backend);
        assert_eq!(r.window.frames, 2);
        assert_eq!(r.window.presents, [8, 9]);
    }

    #[test]
    fn an_entry_outside_every_producer_is_unknown_and_outside_a_frame_is_outside_render() {
        let mut c = open(true, 1);
        c.call(
            &entry("D3DResource_AddRef", 0x0012_0000, Tally::default()),
            None,
        );
        c.frame_begin();
        c.call(&entry(DRAW, 0x0002_9b40, tally(1, 0, 1, 4)), None);
        c.frame_end();
        let r = close(c, 8);
        let p = r.producers.as_ref().unwrap();
        assert_eq!(
            p[&Producer::OutsideRender].work.entries,
            2,
            "AddRef and the closing Swap"
        );
        assert_eq!(p[&Producer::Unknown].work.draws, 1);
        assert_eq!(Producer::Unknown.status(), Status::Unknown);
        assert_eq!(r.unattributed, (1, 1));
    }

    /// The frame wait calls no D3D entry; a resource call another thread
    /// makes while it yields is not the frame's, so a frame drawn natively
    /// adds nothing outside `outside_render` across its wait.
    #[test]
    fn the_frame_wait_is_outside_the_render() {
        let mut c = open(true, 1);
        c.frame_begin();
        c.wait(true);
        c.call(&entry("D3DResource_IsBusy", 0x0012_c9a0, Tally::default()), None);
        c.call(&entry("D3D_BlockOnResource", 0x0012_c9c0, Tally::default()), None);
        c.wait(false);
        c.call(&entry(DRAW, 0x0002_9b40, tally(1, 0, 1, 4)), None);
        c.wait(true);
        c.frame_end();
        c.call(&entry("D3DResource_AddRef", 0x0012_0000, Tally::default()), None);
        let r = close(c, 8);
        let p = r.producers.as_ref().unwrap();
        assert_eq!(p[&Producer::OutsideRender].work.entries, 4, "the wait's two, AddRef after the frame and the Swap");
        assert_eq!(p[&Producer::OutsideRender].work.locks, 2);
        assert_eq!(p[&Producer::Unknown].work.entries, 1, "the draw after the wait is the frame's");
    }

    /// The frame's own calls have rows: inside the frame step the views'
    /// setup and the frame outside a view are not `unknown`, a kept
    /// frame's `Swap` is the present's, and a frame loop outside the
    /// frame step files its view and present under their rows, not
    /// `outside_render`.
    #[test]
    fn the_frames_own_calls_are_named_rows_inside_and_outside_the_frame_step() {
        let producer = |name| Producer::Fixed(row(name).unwrap());
        let mut c = open(true, 2);
        c.frame_begin();
        for name in ["views", "outside_view"] {
            let p = producer(name);
            c.enter(p);
            c.call(&entry("D3DDevice_SetRenderTarget", 0x0002_c8f0, Tally::default()), None);
            c.leave(|q| q == p);
        }
        let present = producer("present");
        c.enter(present);
        c.call(&swap(8), None);
        c.leave(|q| q == present);
        c.frame_end();
        // A frame loop: not a render frame.
        let (looped, outside) = (producer("frame_loop"), producer("outside_view"));
        c.enter(looped);
        c.call(&entry("D3DDevice_Clear", 0x0002_ca40, Tally::default()), None);
        c.leave(|q| q == looped);
        c.enter(outside);
        c.call(&entry("D3DDevice_SetViewport", 0x0002_c900, Tally::default()), None);
        c.leave(|q| q == outside);
        c.enter(present);
        let r = close(c, 9);
        let p = r.producers.as_ref().unwrap();
        assert_eq!(p[&producer("views")].work.binds, 1);
        assert_eq!(p[&outside].work.entries, 2, "one in the frame step, one in the loop");
        assert_eq!(p[&looped].work.clears, 1);
        assert_eq!(p[&present].work.presents, 2, "the kept frame's Swap and the loop's");
        assert!(!p.contains_key(&Producer::Unknown), "nothing inside the frame is unknown");
        assert!(!p.contains_key(&Producer::OutsideRender), "nothing of the loop is outside the render");
        assert_eq!(present.status(), Status::Kept);
        for name in ["views", "outside_view", "frame_loop"] {
            assert_eq!(producer(name).status(), Status::D3d, "{name}");
        }
    }

    #[test]
    fn without_observers_unattributed_holds_the_inline_draws() {
        let mut c = open(false, 1);
        let call = Call {
            inline: tally(3, 0, 3, 40),
            ..entry(DRAW, 0x0004_0000, tally(1, 0, 1, 4))
        };
        c.call(&call, None);
        let r = close(c, 8);
        assert_eq!(r.unattributed, (3, 0));
        assert_eq!(r.sites[&0x0004_0000].inline_draws, 3);
        assert_eq!(r.totals.draws, 4);
    }

    #[test]
    fn a_window_starts_at_the_next_present_and_rearming_replaces_it() {
        let mut c = Census::new(false, false);
        c.arm(2);
        // Before the first present nothing counts.
        c.call(&entry(DRAW, 0x10, tally(1, 0, 1, 0)), None);
        c.call(&swap(3), None);
        c.call(&entry(DRAW, 0x10, tally(1, 0, 1, 0)), None);
        c.arm(1);
        assert_eq!(c.state(), State::Armed);
        c.call(&swap(4), None);
        c.call(&entry(DRAW, 0x20, tally(1, 0, 1, 0)), None);
        c.call(&swap(5), None);
        let r = c.last().unwrap();
        assert_eq!(r.totals.draws, 1);
        assert!(!r.sites.contains_key(&0x10));
        assert_eq!(r.window.presents, [5, 5]);
        assert_eq!(c.state().name(), "done");
    }

    #[test]
    fn missed_exits_are_dropped_and_counted() {
        let shadow = Producer::Fixed(row("object_shadow").unwrap());
        let mut c = open(true, 1);
        c.frame_begin();
        c.enter(shadow);
        c.enter(Producer::Layer { at: 4, layer: 10 });
        assert_eq!(c.around(), None);
        // The layer's exit never ran.
        c.leave(|p| p == shadow);
        assert_eq!(c.current(), Producer::Unknown);
        c.leave(|p| p == shadow);
        c.enter(shadow);
        c.frame_end();
        let r = close(c, 8);
        assert_eq!(r.unbalanced, 3);
    }

    #[test]
    fn the_opaque_blocks_layer_draws_are_kept_the_others_a_stage_draws_partial_and_the_rest_d3d() {
        let status = |at: u8, layer: u8| Producer::Layer { at, layer }.status().name();
        for layer in [1, 3, 5, 7, 8, 20] {
            assert_eq!(status(0, layer), "kept", "the scene's layer {layer}");
        }
        for layer in [2, 6, 16, 17] {
            assert_eq!(status(0, layer), "kept", "the scene's layer {layer}");
        }
        for layer in [18, 19, 21] {
            assert_eq!(status(6, layer), "kept", "the sorted handlers' layer {layer}");
            assert_eq!(status(5, layer), "d3d", "another caller's layer {layer}");
        }
        assert_eq!(Producer::Layer { at: 6, layer: 18 }.slot(), "transparents");
        for layer in [1, 3, 7, 8, 20] {
            assert_eq!(status(2, layer), "kept", "the first person's layer {layer}");
        }
        assert_eq!(status(2, 16), "partial", "the first person's layer 16");
        assert_eq!(status(2, 2), "kept", "the first person's layer 2");
        assert_eq!(status(2, 5), "d3d", "the first person draws no layer 5");
        for layer in [6, 16] {
            assert_eq!(status(5, layer), "d3d", "a different caller keeps its draw");
        }
        for layer in [1, 2, 3, 5] {
            assert_eq!(status(1, layer), "kept");
        }
        assert_eq!(status(1, 7), "d3d");
        for layer in [12, 13] {
            assert_eq!(status(3, layer), "partial", "the light passes'");
        }
        assert_eq!(status(4, 10), "partial");
        assert_eq!(status(4, 11), "partial");
        for &(_, layers) in &KEPT_LAYERS {
            let returns = |l: &u8| {
                let l = u32::from(*l);
                use crate::frame_step::layer::{FOG_WATER_LAYERS, LATE_LAYERS, LAYERS};
                LAYERS.contains(&l) || LATE_LAYERS.contains(&l) || FOG_WATER_LAYERS.contains(&l)
            };
            assert!(layers.iter().all(returns), "a kept layer's draw can return");
        }
        assert_eq!(status(4, 14), "d3d");
        for r in REGISTRY {
            assert_eq!(
                r.status,
                if matches!(r.name, "object_shadows" | "first_person" | "dynamic_lights" | "light_passes") {
                    Status::Native
                } else if r.name == "present" {
                    Status::Kept
                } else if matches!(
                    r.name,
                    "object_setup"
                        | "game_view"
                        | "alternate_view"
                        | "scene_passes"
                        | "object_shadow"
                        | "water_teardown"
                        | "sky"
                        | "scene_clear"
                        | "target_select"
                        | "transparents"
                        | "screen_effects"
                        | "colour_pass"
                        | "screen_effect"
                        | "hud"
                        | "ui"
                        | "fade"
                        | "loading"
                        | "decals"
                        | "frame_tail"
                        | "movie"
                ) {
                    Status::Partial
                } else {
                    Status::D3d
                },
                "unexpected fixed producer status for {}",
                r.name
            );
        }
        let sky = Producer::Fixed(row("sky").unwrap());
        assert_eq!(sky.status(), Status::Partial);
        assert_eq!(sky.slot(), "after_lights");
    }

    #[test]
    fn scene_attachment_helpers_count_device_entries_under_the_open_helper() {
        let mut c = open(true, 1);
        c.frame_begin();
        for (name, device_entry, tally) in [
            ("scene_clear", "D3DDevice_Clear", Tally { native_rect_clears: 1, ..Tally::default() }),
            ("target_select", "D3DDevice_SetRenderTarget", Tally { guest_bind_refused_destroy: 1, ..Tally::default() }),
        ] {
            let helper = Producer::Fixed(row(name).unwrap());
            c.enter(helper);
            c.call(&entry(device_entry, 0x1100, tally), None);
            c.leave(|p| p == helper);
        }
        c.frame_end();
        let rows = close(c, 8).producers.unwrap();
        let clear = rows[&Producer::Fixed(row("scene_clear").unwrap())].work;
        let bind = rows[&Producer::Fixed(row("target_select").unwrap())].work;
        assert_eq!((clear.clears, clear.binds, clear.native_rect_clears), (1, 0, 1));
        assert_eq!((bind.clears, bind.binds, bind.guest_bind_refused_destroy), (0, 1, 1));
    }

    #[test]
    fn screen_effects_charge_claimed_and_guest_draws_to_the_partial_producer() {
        let producer = |name| Producer::Fixed(row(name).unwrap());
        let parent = producer("screen_effects");
        let mut c = open(true, 1);
        c.frame_begin();
        c.enter(parent);
        for name in ["colour_pass", "screen_effect"] {
            let child = producer(name);
            c.enter(child);
            c.call(&entry(DRAW, 0x1200, tally(3, 2, 1, 0)), None);
            c.leave(|p| p == child);
        }
        c.call(&entry(DRAW, 0x1300, tally(1, 0, 1, 0)), None);
        c.leave(|p| p == parent);
        let fade = producer("fade");
        c.enter(fade);
        c.call(&entry(DRAW, 0x1400, tally(1, 1, 0, 0)), None);
        c.leave(|p| p == fade);
        c.frame_end();
        let rows = close(c, 8).producers.unwrap();
        for (name, expected) in [
            ("screen_effects", (1, 0, 1)),
            ("colour_pass", (3, 2, 1)),
            ("screen_effect", (3, 2, 1)),
            ("fade", (1, 1, 0)),
        ] {
            let p = producer(name);
            let w = rows[&p].work;
            assert_eq!(p.status(), Status::Partial, "{name}");
            assert_eq!((w.draws, w.claimed, w.residual), expected, "{name}");
        }
    }

    #[test]
    fn every_registry_row_is_named_once_and_has_a_slot() {
        for (i, r) in REGISTRY.iter().enumerate() {
            assert_eq!(row(r.name), Some(i), "{}", r.name);
            assert!(!r.slot.is_empty(), "{}", r.name);
            assert_eq!(Producer::Fixed(i).name(), r.name);
        }
        assert_eq!(
            Kind::of("D3DDevice_SetRenderTarget (same size, 0x3f7b30)"),
            Kind::Bind
        );
        assert_eq!(Kind::of("D3DResource_IsBusy (0x3fb710)"), Kind::Lock);
        assert_eq!(Kind::of("D3DDevice_DrawVertices"), Kind::Other);
    }
}
