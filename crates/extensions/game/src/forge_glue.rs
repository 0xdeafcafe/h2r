//! Local Forge session lifetime. The player and agent share one document and
//! apply queue. A map activation invalidates handles, outstanding calls and
//! any map file still being read by the background worker.
mod editing;
pub mod gizmo;
pub mod ground;
mod link;
pub mod marker;
pub mod markers_world;
pub mod net;

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

use agent::forge::local::{self, LocalApi, ProposalReply};
use agent::sim::calls;
use forge::look::{
    Refusal,
    census::{self, Entry as Model},
    monitors::{Looks, Who},
};
use forge_doc::{Entry, ForgeDoc, Op, Sha256};
use guest_layout::game::{GameGlobals, GameTime};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use serde_json::{Value, json};

use crate::forge_controls as controls;
use crate::forge_map::{self, Loaded, Opened};
use crate::ui_kit::spec::ScreenSpec;
use editing::Editor;

/// Ticks between rebuilds of the game type markers.
const MARKS_EVERY: u32 = 3;

type Shared = Arc<Mutex<Driver>>;
type MapResult = Result<Opened, String>;

static CURRENT: Mutex<Weak<Mutex<Driver>>> = Mutex::new(Weak::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The saved map the next session loads as it opens, asked for by an agent
/// (`None`: none asked). A game's start does not clear it; the session that
/// opens takes it, else the lobby's Forge Map row's choice
/// ([`crate::lobby_start::forge_map`]).
static DRAFT: Mutex<Option<String>> = Mutex::new(None);

/// Asks the next session to load the saved map `name` as it opens, or none.
pub fn request_draft(name: Option<String>) {
    *DRAFT.lock().unwrap_or_else(|e| e.into_inner()) = name;
}

fn take_draft() -> Option<String> {
    DRAFT.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub fn requested() -> Result<bool, String> {
    Ok(hooks::Switches::from_env()?.feature("forge", true))
}

fn current() -> Option<Shared> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).upgrade()
}

/// The window draws this page over the current world frame.
pub fn page() -> Option<ScreenSpec> {
    let driver = current()?;
    let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    driver.editor.as_ref()?.page()
}

/// The rings the play window's overlay paints beside the gizmo, projected
/// through the last frame's world view: the session's other monitors. A
/// monitor's own marker is never drawn. The ground's marks and problems are
/// game type markers ([`markers_world`]).
pub fn markers() -> Vec<marker::Marker> {
    let Some(driver) = current() else {
        return Vec::new();
    };
    let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    driver
        .markers
        .iter()
        .filter_map(|(frame, colour)| marker::project(*frame, *colour))
        .collect()
}

/// The other seats' drawn bodies, as the last tick left them: the
/// presentation instances the native renderer's object pass draws. The
/// local seat's own body is never drawn in its own view.
pub fn monitor_bodies() -> Vec<forge::look::body::Draw> {
    let Some(driver) = current() else {
        return Vec::new();
    };
    driver.lock().unwrap_or_else(|e| e.into_inner()).bodies.clone()
}

/// World-space axes and the held object's outline, projected through the
/// render frame's camera.
pub fn gizmo() -> Option<gizmo::Axes> {
    let (frame, extent, local) = {
        let driver = current()?;
        let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
        let editor = driver.editor.as_ref()?;
        let (frame, extent) = editor.ghost()?;
        (frame, extent, editor.local_axes())
    };
    gizmo::project(frame, extent, local)
}

/// A click on a compositor dialog's row. The Forge screens are the game's
/// (`crate::forge_screens`), which take the mouse as the game does, so the
/// compositor draws no dialog and no click is the session's.
pub fn click(_row: usize) -> bool {
    false
}

fn admitted(m: &dyn engine::GuestMemory, generation: u64) -> bool {
    GENERATION.load(Ordering::Acquire) == generation && forge_map::admitted_session(m)
}

struct Driver {
    /// The monitor looks: this machine's catalogue and, while a session is
    /// open, the local seat's look (docs/game/forge-monitor-appearance.md).
    looks: Looks,
    store: Option<Arc<settings::Store>>,
    calls: calls::Shared,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    data: PathBuf,
    /// The co-op link a shared session's lanes run over, offered each
    /// tick before anything else the driver does.
    link: link::CoopLink,
    loaded: Option<Loaded>,
    opening: Option<Receiver<MapResult>>,
    editor: Option<Editor>,
    /// The saved map the editor loads at its first tick.
    draft: Option<String>,
    /// The other seats' markers as the last tick left them, for the play
    /// window's overlay ([`markers`]).
    markers: Vec<(forge::Frame, [u8; 3])>,
    /// The game type markers the last rebuild made, and the tick of it
    /// ([`markers_world`]).
    marks: Arc<Vec<markers_world::Entry>>,
    marks_at: Option<u32>,
    /// The other seats' drawn bodies as the last tick left them, for the
    /// native renderer's object pass ([`monitor_bodies`]).
    bodies: Vec<forge::look::body::Draw>,
    reason: String,
}

impl Driver {
    fn reset(&mut self) {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        self.editor = None;
        self.draft = None;
        self.looks.close();
        self.opening = None;
        self.loaded = None;
        self.markers.clear();
        self.marks = Arc::default();
        self.marks_at = None;
        markers_world::clear();
        self.bodies.clear();
        self.link.reset();
        self.reason = "Waiting for a local multiplayer map.".into();
        controls::publish(controls::State::default());
        crate::forge_screens::publish(crate::forge_screens::Facts::default());
        crate::forge_readout::clear();
        crate::forge_hud::clear();
    }

    fn open(&mut self, loaded: &Loaded) -> Result<(), String> {
        let vfs = self
            .vfs
            .clone()
            .ok_or("Forge needs the mounted map files.")?;
        let (send, recv) = mpsc::sync_channel(1);
        let source = loaded.clone();
        background::spawn("forge-map", move || {
            let _ = send.send(forge_map::open(&vfs, &source));
        })
        .map_err(|e| e.to_string())?;
        self.opening = Some(recv);
        self.reason = "Reading the Forge palette.".into();
        Ok(())
    }

    fn tick(&mut self, m: &dyn engine::GuestMemory) {
        // The co-op link offers its lanes before anything else, so a
        // session that opens this tick finds them.
        self.link.tick(m);
        if self
            .loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.matches(m))
        {
            self.reset();
        }
        if self.loaded.is_none() {
            if !forge_map::admitted_session(m) {
                return;
            }
            // The monitor is a Forge lobby's (or the level boot's) only
            // (docs/game/forge-firefight-ui.md, "The lobbies").
            if !crate::lobby_start::forge_session() {
                self.reason = "The lobby's mode is not Forge.".into();
                return;
            }
            let loaded = match Loaded::read(m, false) {
                Ok(loaded) => loaded,
                Err(reason) => {
                    self.reason = reason;
                    return;
                }
            };
            let result = self.open(&loaded);
            self.loaded = Some(loaded);
            if let Err(e) = result {
                self.reason = e;
            }
        }
        let loaded = self.loaded.as_ref().expect("read above");
        if let Some(recv) = &self.opening {
            let result = match recv.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("The Forge map reader stopped.".into()),
            };
            self.opening = None;
            match result {
                Ok(mut opened) => {
                    // The map's own entries, read once from the running
                    // scenario before any edit (docs/game/forge-map-variants.md,
                    // "Applying an edit in a session").
                    let rows = forge_map::rows::read(m, loaded.scenario, &|datum: u32| {
                        opened.names.get(&datum).cloned()
                    });
                    opened.session = opened.session.with_base(rows.into_base());
                    for line in opened.session.left_out() {
                        eprintln!("forge: the map's things leave out {line}");
                    }
                    self.draft = take_draft().or_else(crate::lobby_start::forge_map);
                    let mut editor =
                        Editor::new(opened, &self.data, loaded.scenario, self.calls.clone());
                    let tick = GameTime::current(m)
                        .ok()
                        .flatten()
                        .and_then(|t| t.tick(m).ok())
                        .unwrap_or_default();
                    if let Some(notice) = self.looks.open(self.store.as_deref(), &who_of(Some(m))) {
                        editor.say(notice, tick);
                    }
                    // A synchronous network game's session is shared over
                    // the Forge lanes its link offered.
                    if forge_map::admitted_shared(m) {
                        let Some(offer) = net::take() else {
                            self.reason = "The co-op link has no Forge channel.".into();
                            return;
                        };
                        let name = who_of(Some(m)).profile;
                        let monitor = monitor_start(m, &self.looks);
                        if let Err(e) = editor.open_shared(offer, tick, &name, &monitor) {
                            self.reason = e;
                            return;
                        }
                    }
                    self.editor = Some(editor);
                }
                Err(error) => {
                    eprintln!("forge: {error}");
                    self.reason = error;
                    return;
                }
            }
        }
        if let Some(e) = self.editor.as_mut()
            && let Some(tick) = GameTime::current(m)
                .ok()
                .flatten()
                .and_then(|t| t.tick(m).ok())
        {
            let bsp = GameGlobals::current(m)
                .ok()
                .flatten()
                .and_then(|g| g.active_bsp(m).ok())
                .and_then(|i| usize::try_from(i).ok());
            if let Some(name) = self.draft.take() {
                e.load_draft(tick, name);
            }
            // The session's game type decides which things the tool aims
            // at (docs/game/forge-map-variants.md, "Game types").
            let game = GameGlobals::current(m)
                .ok()
                .flatten()
                .and_then(|g| g.options().variant().engine(m).ok());
            e.tick(tick, bsp, crate::forge_screens::ui_up(m), game);
            let now = tick as f32 / 30.;
            crate::forge_readout::publish(e.readout_scene(game));
            crate::forge_hud::publish(e.monitor_hud());
            self.markers = e.monitor_markers(now);
            // The things are rebuilt a few times a second; the aimed and
            // held things move the selection every tick.
            if self
                .marks_at
                .is_none_or(|at| tick < at || tick - at >= MARKS_EVERY)
            {
                self.marks = Arc::new(e.marker_entries());
                self.marks_at = Some(tick);
            }
            let (aimed, held) = e.marker_pins();
            markers_world::publish(&self.marks, aimed, held);
            self.bodies = e.monitor_draws(now, now);
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.reset();
        controls::forget_park();
    }
}

fn manifest() -> Manifest {
    Manifest::new("forge", "Local Forge monitor and Objects tool").doc("docs/game/forge-design.md")
}

/// Whether the `forge` feature is on under `switches`, by its manifest's
/// default: the lobby offers the Forge mode with it.
pub fn enabled(switches: &hooks::Switches) -> bool {
    let m = manifest();
    switches.feature(m.id, m.default_on)
}

struct Hooks(Shared);

impl Feature for Hooks {
    fn manifest(&self) -> Manifest {
        manifest()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        controls::camera_hooks(b)?;
        crate::forge_readout::hook(b)?;
        crate::forge_hud::hook(b)?;
        let driver = self.0.clone();
        b.game().on_start("reset the Forge document", move |_| {
            controls::forget_park();
            driver.lock().unwrap_or_else(|e| e.into_inner()).reset();
        });
        let driver = self.0.clone();
        b.game().on_end("release the Forge monitor", move |o| {
            let guest = o.guest();
            let m: &dyn engine::GuestMemory = &*guest;
            if GameGlobals::current(m)
                .ok()
                .flatten()
                .and_then(|g| g.game_ended(m).ok())
                .is_some_and(|v| v != 0)
            {
                driver.lock().unwrap_or_else(|e| e.into_inner()).reset();
                net::clear();
            }
        });
        let driver = self.0.clone();
        b.frame().on_tick("apply Forge input", move |o| {
            driver
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tick(&*o.guest());
        });
        let driver = self.0.clone();
        b.snapshot(SnapshotPolicy::forbid_while(move || {
            let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
            (driver.opening.is_some() || driver.editor.is_some())
                .then(|| "The local Forge session owns a live edit log.".into())
        }));
        Ok(())
    }
}

/// What the session's monitor channel starts with (docs/game/
/// forge-monitor-appearance.md, ML7): the local seat's look, what this
/// machine can draw as its hello says, the host model list's catalogue
/// and the palette's RGB.
fn monitor_start(m: &dyn engine::GuestMemory, looks: &Looks) -> editing::MonitorStart {
    let colour = crate::pad_lightbar::seat_colour(m, 0).unwrap_or(0);
    let resolved = looks
        .session()
        .cloned()
        .unwrap_or_else(|| forge::look::MonitorLook::default_for(colour));
    let catalogue = looks.catalogue().to_vec();
    let base = catalogue
        .iter()
        .find(|e| e.key == resolved.model)
        .filter(|_| !resolved.is_marker())
        .map(|e| e.sha256.clone());
    editing::MonitorStart {
        base,
        can: forge::look::session::CanDraw::ours(
            forge::look::BODIES_DRAWN,
            catalogue.iter().map(|e| e.sha256.clone()).collect(),
        ),
        look: resolved,
        catalogue,
        palette: (0..forge::look::PALETTE)
            .map(|index| crate::pad_lightbar::palette_rgb(m, index).unwrap_or([0; 3]))
            .collect(),
    }
}

/// The profile of local slot 0 and its colour: whose monitor look this is.
/// Without a signed-in profile it is the default profile
/// ([`crate::skulls::DEFAULT_PROFILE`]) and palette colour 0.
fn who_of(m: Option<&dyn engine::GuestMemory>) -> Who {
    match m {
        Some(m) => Who {
            profile: crate::skulls::profile_or_default(&crate::layout_glue::ReadGm(m), 0),
            colour: crate::pad_lightbar::seat_colour(m, 0).unwrap_or(0),
        },
        None => Who {
            profile: crate::skulls::DEFAULT_PROFILE.to_owned(),
            colour: 0,
        },
    }
}

/// This machine's catalogue of monitor models: the census `h2 forge looks`
/// last wrote under the data root, or empty with the reason.
fn catalogue(data: &Path) -> (Vec<Model>, Option<String>) {
    match census::load_latest(&data.join("cache")) {
        Ok(Some(c)) => (c.entries, None),
        Ok(None) => (
            Vec::new(),
            Some(
                "No census yet: run `h2 forge looks` to list the models this install can draw"
                    .into(),
            ),
        ),
        Err(e) => (Vec::new(), Some(e)),
    }
}

pub fn install(
    registry: &mut hooks::Registry,
    agent: Option<&crate::agent_glue::AgentGlue>,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    data: &Path,
    store: Option<Arc<settings::Store>>,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    let calls = crate::lobby_start::install_queue(registry, agent)?;
    let (models, note) = catalogue(data);
    let driver = Arc::new(Mutex::new(Driver {
        looks: Looks::new(models, note),
        store,
        calls,
        vfs,
        data: data.into(),
        link: link::CoopLink::new(Box::new(link::HleTransport::new(live))),
        loaded: None,
        opening: None,
        editor: None,
        draft: None,
        markers: Vec::new(),
        marks: Arc::default(),
        marks_at: None,
        bodies: Vec::new(),
        reason: "Waiting for a local multiplayer map.".into(),
    }));
    if registry
        .add(&Hooks(driver.clone()))
        .map_err(anyhow::Error::msg)?
    {
        controls::publish(controls::State::default());
        crate::forge_screens::install(registry)?;
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&driver);
        if let Some(a) = agent {
            local::install(&a.agent, Backend(driver));
        }
    }
    Ok(())
}

struct Backend(Shared);

impl Backend {
    fn read<T>(&self, f: impl FnOnce(&Editor) -> T) -> Result<T, String> {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        driver
            .editor
            .as_ref()
            .map(f)
            .ok_or_else(|| driver.reason.clone())
    }
}

impl LocalApi for Backend {
    fn who(&self, m: Option<&dyn engine::GuestMemory>) -> Who {
        who_of(m)
    }

    fn look(&self, who: &Who, seat: Option<u64>) -> Result<Value, Refusal> {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        driver.looks.report(driver.store.as_deref(), who, seat)
    }

    fn look_set(&self, who: &Who, params: Value) -> Result<Value, Refusal> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let store = driver.store.clone();
        driver.looks.set(store.as_deref(), who, &params)
    }

    fn monitors(&self, who: &Who) -> Value {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (monitor, pose) = driver
            .editor
            .as_ref()
            .map_or((false, None), |e| (e.monitor(), e.pose()));
        driver.looks.monitors(who, monitor, pose)
    }

    fn status(&self) -> Result<Value, String> {
        Ok(self
            .read(Editor::status)
            .unwrap_or_else(|reason| json!({"session":"inactive","reason":reason})))
    }

    fn session(&self) -> Result<Arc<forge::Session>, String> {
        self.read(|e| e.session.clone())
    }

    fn ground(&self) -> Result<Value, String> {
        self.read(Editor::ground_json)
    }

    fn ground_build(&self, if_stale: bool) -> Result<local::GroundReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.ground_build(if_stale)
    }

    fn document(&self) -> Result<ForgeDoc, String> {
        self.read(|e| e.session.document().clone())
    }

    fn log(&self, since: u32) -> Result<Vec<Entry>, String> {
        self.read(|e| e.log_since(since))
    }

    fn wait_ticks(&self) -> u64 {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if driver.editor.as_ref().is_some_and(Editor::is_shared) {
            agent::forge::net::WAIT_TICKS
        } else {
            30
        }
    }

    fn input(&self, m: &dyn engine::GuestMemory, input: forge::tool::Input) -> Result<(), String> {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if driver.editor.is_none()
            || !driver
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.matches(m))
        {
            return Err("Forge input requires the active local session.".into());
        }
        controls::enqueue(input)
    }

    fn propose(&self, tick: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.propose(tick, op, was)
    }
}
