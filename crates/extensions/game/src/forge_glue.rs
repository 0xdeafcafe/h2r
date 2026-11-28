//! Local Forge session lifetime. The player and agent share one document and
//! apply queue. A map activation invalidates handles, outstanding calls and
//! any map file still being read by the background worker, and ends the
//! session, but for a reload of the session's own (**Rebuild map**, the
//! reload after **Build AI ground**): the driver carries the editor across
//! it and the session goes on over the reloaded map ([`reload`]).
mod editing;
pub mod gizmo;
pub mod ground;
mod link;
pub mod markers_world;
pub mod nav;
pub mod nav_world;
pub mod net;
pub mod pieces_world;
pub mod reload;

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

/// The other seats' drawn bodies, as the last tick left them: the
/// presentation instances the native renderer's object pass draws. The
/// local seat's own body is never drawn in its own view.
pub fn monitor_bodies() -> Vec<forge::look::body::Draw> {
    let Some(driver) = current() else {
        return Vec::new();
    };
    driver.lock().unwrap_or_else(|e| e.into_inner()).bodies.clone()
}

/// Whether this machine draws a disc model for a monitor: the build has the
/// bodies stage (`forge::look::BODIES_DRAWN`) and the native renderer is on
/// (`monitor_model`). Without either, a model look is held and the marker
/// shows with the reason.
pub fn bodies_drawn() -> bool {
    forge::look::BODIES_DRAWN && cfg!(feature = "gpu")
}

/// What this machine draws as a monitor: the renderer's declaration and
/// the source maps its catalogue holds. A film's recorded looks are measured
/// against it (`theater::forge_film`, `monitor_look_source`); without a
/// running Forge driver the catalogue is empty and every model look is
/// drawn as the marker.
pub fn can_draw() -> forge::look::session::CanDraw {
    let maps = current()
        .map(|driver| -> Vec<String> {
            let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
            driver.looks.catalogue().iter().map(|e| e.sha256.clone()).collect()
        })
        .unwrap_or_default();
    forge::look::session::CanDraw::ours(bodies_drawn(), maps)
}

/// The bodies a film's monitor records give at the film tick Theater plays
/// (docs/game/forge-monitor-appearance.md, "Theater"): the seats' looks and
/// poses drawn by the code the live session uses, from this machine's
/// catalogue. Empty outside a film that recorded monitors.
pub fn playback_bodies(m: &dyn guest_layout::GuestMemory) -> Vec<forge::look::body::Draw> {
    if !crate::theater::forge_film::active() {
        return Vec::new();
    }
    let catalogue = current()
        .map(|driver| {
            let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
            driver.looks.catalogue().to_vec()
        })
        .unwrap_or_default();
    let bodies = forge::look::body::Bodies::new(&catalogue);
    crate::theater::forge_film::draws(&bodies, |index| {
        crate::pad_lightbar::palette_rgb(m, index).unwrap_or([0; 3])
    })
}

/// The monitor the game's Appearance page draws beside the Spartan in
/// `widget` (docs/game/forge-monitor-appearance.md, ML4b): the saved look
/// of the profile on local slot 0, as this machine's catalogue can draw
/// it. `None` leaves the page as retail draws it.
pub fn appearance_body(
    m: &dyn guest_layout::GuestMemory,
    widget: &forge::look::appearance::Widget,
    seconds: f32,
) -> Option<forge::look::body::Draw> {
    let who = who_of(Some(m));
    let driver = current()?;
    let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    let saved = forge::look::saved::load(driver.store.as_deref()?, &who.profile, who.colour).saved;
    forge::look::appearance::beside(
        widget,
        Some(&saved),
        driver.looks.catalogue(),
        0,
        seconds,
        |index| crate::pad_lightbar::palette_rgb(m, index).unwrap_or([0; 3]),
    )
}

/// The host lights of the other seats' bodies (docs/game/
/// forge-monitor-appearance.md, ML5), in the palette colours the loaded
/// map holds: what the frame step's scene adds to each game view after
/// the game's candidates (`render_bridge::scene_step::set_host_lights`).
pub fn monitor_lights(m: &dyn guest_layout::GuestMemory) -> Vec<frame_scene::Light> {
    let mut bodies = monitor_bodies();
    bodies.extend(playback_bodies(m));
    // The same per-view call hands the frame graph the bodies themselves
    // (ML4): the frame's models and poses, drawn after the game's view.
    #[cfg(feature = "gpu")]
    publish_bodies(&bodies);
    host_lights(&bodies, |index| crate::pad_lightbar::palette_rgb(m, index))
}

/// The bodies of the frame, with their models, to the frame graph
/// (`monitor_model::gpu::publish`). A machine with no Forge driver, or no
/// body, publishes none.
#[cfg(feature = "gpu")]
fn publish_bodies(bodies: &[forge::look::body::Draw]) {
    let catalogue = current()
        .map(|driver| {
            let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
            driver.looks.catalogue().to_vec()
        })
        .unwrap_or_default();
    if bodies.is_empty() || catalogue.is_empty() {
        crate::monitor_model::gpu::clear();
    } else {
        crate::monitor_model::gpu::publish(bodies, &catalogue);
    }
}

/// The census entry the host model list names `id` (a
/// `forge::look::body::Draw::model`): the catalogue's `id`th, as
/// `Bodies::new` lists it.
pub fn catalogue_entry(id: u32) -> Option<Model> {
    let driver = current()?;
    let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    driver.looks.catalogue().get(id as usize).cloned()
}

/// The host light each drawn body carries: at the body's origin, the
/// look's light colour from `rgb`, its radius, no shadow. A marker draws
/// no light, and neither does a body whose colour the map does not hold.
pub fn host_lights(
    draws: &[forge::look::body::Draw],
    rgb: impl Fn(u8) -> Option<[u8; 3]>,
) -> Vec<frame_scene::Light> {
    draws
        .iter()
        .filter(|d| d.model.is_some())
        .filter_map(|d| {
            let light = d.light?;
            let [r, g, b] = rgb(light.colour)?;
            let colour = [r, g, b].map(|c| f32::from(c) / 255.0);
            Some(frame_scene::Light::host_light(
                d.seat,
                d.frame.position,
                colour,
                light.radius,
            ))
        })
        .collect()
}

fn admitted(m: &dyn guest_layout::GuestMemory, generation: u64) -> bool {
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
    /// The game type markers the last rebuild made, and the tick of it
    /// ([`markers_world`]).
    marks: Arc<Vec<markers_world::Entry>>,
    marks_at: Option<u32>,
    marks_preview: bool,
    /// The other seats' drawn bodies as the last tick left them, for the
    /// native renderer's object pass ([`monitor_bodies`]).
    bodies: Vec<forge::look::body::Draw>,
    reason: String,
    /// The reload of the running map onto a play copy, when the game has
    /// one (the agent's `map.reload`).
    reloader: Option<Box<dyn reload::Reloader>>,
    /// A reload of ours under way, carrying the editor across the map's
    /// activation.
    carry: Option<reload::Carry>,
}

impl Driver {
    fn reset(&mut self) {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        // A reload of ours keeps the editor for the session that opens on
        // the copy; any other change of map ends it, its wave test first.
        let mut editor = self.editor.take();
        match &mut self.carry {
            Some(carry) if carry.editor.is_none() => carry.editor = editor,
            _ => {
                if let Some(e) = editor.as_mut() {
                    e.leave();
                }
            }
        }
        self.draft = None;
        self.looks.close();
        self.opening = None;
        self.loaded = None;
        self.marks = Arc::default();
        self.marks_at = None;
        self.marks_preview = false;
        markers_world::clear();
        pieces_world::clear();
        nav_world::clear();
        nav::unstage();
        self.bodies.clear();
        #[cfg(feature = "gpu")]
        crate::monitor_model::gpu::clear();
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

    /// A session over a map opened as the host opens one: its own entries
    /// read from the running scenario, and the lobby's draft to load.
    /// `why` is the notice of a reload the session could not go on over.
    fn fresh(
        &mut self,
        mut opened: Opened,
        loaded: &Loaded,
        m: &dyn guest_layout::GuestMemory,
        tick: u32,
        why: Option<String>,
    ) -> Editor {
        // The map's own entries, read once from the running scenario before
        // any edit (docs/game/forge-map-variants.md, "Applying an edit in a
        // session").
        let rows = forge_map::rows::read(m, loaded.scenario, &|datum: u32| {
            opened.names.get(&datum).cloned()
        });
        opened.session = opened.session.with_base(rows.into_base());
        for line in opened.session.left_out() {
            eprintln!("forge: the map's things leave out {line}");
        }
        self.draft = take_draft().or_else(crate::lobby_start::forge_map);
        let mut editor = Editor::new(opened, &self.data, loaded.scenario, self.calls.clone());
        editor.set_map_name(loaded.name.clone());
        // The body D-pad Up swaps into: the profile's choice
        // (docs/game/forge-design-slices.md, "The monitor body").
        if let Some(store) = self.store.as_deref() {
            let (body, problem) = forge::body::load(store, &who_of(Some(m)).profile);
            editor.set_body(body);
            if let Some(problem) = problem {
                eprintln!("forge: {problem}");
            }
        }
        if let Some(why) = why {
            editor.say(why, tick);
        }
        editor
    }

    /// Ask a reload of ours how it stands. A refused one leaves the editor
    /// on the map it has, saying why.
    fn poll_reload(&mut self, m: &dyn guest_layout::GuestMemory) {
        let Some(carry) = &mut self.carry else { return };
        if let reload::Step::Refused(why) = carry.poll() {
            // A carried editor's map went away for another reason: it ends.
            self.carry = None;
            match self.editor.as_mut() {
                Some(e) => {
                    e.set_reloading(false);
                    e.say(why, tick_of(m).unwrap_or_default());
                }
                None => eprintln!("forge: the map was not reloaded: {why}"),
            }
        }
    }

    fn tick(&mut self, m: &dyn guest_layout::GuestMemory) {
        // The co-op link offers its lanes before anything else, so a
        // session that opens this tick finds them.
        self.link.tick(m);
        self.poll_reload(m);
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
        if let Some(recv) = &self.opening {
            let result = match recv.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("The Forge map reader stopped.".into()),
            };
            self.opening = None;
            match result {
                Ok(opened) => {
                    let loaded = self.loaded.clone().expect("read above");
                    let loaded = &loaded;
                    let tick = tick_of(m).unwrap_or_default();
                    // A reload of ours: the session goes on over the copy.
                    let carried = self
                        .carry
                        .take()
                        .filter(|carry| carry.opens(loaded.checksum));
                    let mut editor = match carried {
                        Some(carry) => match reopen(carry, opened, loaded, m, tick) {
                            Ok(editor) => editor,
                            Err((why, opened)) => {
                                eprintln!("forge: {why}");
                                self.fresh(opened, loaded, m, tick, Some(why))
                            }
                        },
                        None => self.fresh(opened, loaded, m, tick, None),
                    };
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
            // A play copy built: reload the map onto it.
            if let Some(copy) = e.take_copy() {
                let request = match self.reloader.as_mut() {
                    Some(reloader) => reloader.request(m, &copy.file),
                    None => Err(reload::NO_RELOAD.into()),
                };
                match request {
                    Ok(ticket) => {
                        e.set_reloading(true);
                        self.carry = Some(reload::Carry::new(copy, ticket));
                    }
                    Err(why) => e.say(why, tick),
                }
            }
            // The session's game type decides which things the tool aims
            // at (docs/game/forge-map-variants.md, "Game types").
            let game = GameGlobals::current(m)
                .ok()
                .flatten()
                .and_then(|g| g.options().variant().engine(m).ok());
            // The block-out settings of the profile's store, and the ones the
            // player changed on a screen written back to it.
            if let Some(store) = self.store.as_deref() {
                e.set_block_settings(
                    store
                        .get(forge::piece::GRID_KEY)
                        .and_then(settings::Value::as_int)
                        .and_then(|v| u16::try_from(v).ok()),
                    store
                        .get(forge::piece::DEV_VIEW_KEY)
                        .and_then(settings::Value::as_bool),
                );
            }
            e.tick(tick, bsp, crate::forge_screens::ui_up(m), game, m);
            if let Some(store) = self.store.as_deref() {
                for (key, value) in e.take_setting_writes() {
                    if let Err(why) = store.set(key, value) {
                        eprintln!("forge: {key} was not saved: {why}");
                    }
                }
            }
            let now = tick as f32 / 30.;
            crate::forge_readout::publish(e.readout_scene(game));
            crate::forge_hud::publish(e.monitor_hud(now));
            // The things are rebuilt a few times a second; the aimed and
            // held things move the selection every tick.
            let preview = e.marker_preview();
            if preview || self.marks_preview || self
                .marks_at
                .is_none_or(|at| tick < at || tick - at >= MARKS_EVERY)
            {
                self.marks = Arc::new(e.marker_entries());
                self.marks_at = Some(tick);
                pieces_world::publish(&e.piece_draws());
            }
            self.marks_preview = preview;
            let (aimed, held) = e.marker_pins();
            markers_world::publish(&self.marks, aimed, held);
            nav_world::publish(e.nav_lines());
            self.bodies = e.monitor_draws(now, now);
        }
    }
}

/// The game's tick now, if it is ticking.
fn tick_of(m: &dyn guest_layout::GuestMemory) -> Option<u32> {
    GameTime::current(m).ok().flatten().and_then(|t| t.tick(m).ok())
}

/// The session a reload of ours carried, going on over the copy the map
/// was reloaded onto: each added object's live object is the one the game
/// made at its row (`agent::forge::base::live_object`). An `Err` gives
/// `opened` back with why, for a session of its own.
#[allow(clippy::result_large_err)] // `Opened` goes back by value, once per reload.
fn reopen(
    mut carry: reload::Carry,
    opened: Opened,
    loaded: &Loaded,
    m: &dyn guest_layout::GuestMemory,
    tick: u32,
) -> Result<Editor, (String, Opened)> {
    let Some(editor) = carry.editor.take() else {
        return Err(("The reloaded session has no editor.".into(), opened));
    };
    let live = |block, index| {
        agent::forge::base::live_object(m, block, index)
            .ok()
            .flatten()
            .map(|id| id.key)
    };
    let mut editor = editor
        .reopen(opened, loaded.scenario, &carry.copy, live, tick)
        .map_err(|(why, opened)| {
            (format!("The session could not go on over the rebuilt map: {why}"), opened)
        })?;
    editor.set_map_name(loaded.name.clone());
    Ok(editor)
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
            let m: &dyn guest_layout::GuestMemory = &*guest;
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
fn monitor_start(m: &dyn guest_layout::GuestMemory, looks: &Looks) -> editing::MonitorStart {
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
            bodies_drawn(),
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
fn who_of(m: Option<&dyn guest_layout::GuestMemory>) -> Who {
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
        looks: Looks::new(models, note).with_bodies(bodies_drawn()),
        store,
        calls,
        vfs,
        data: data.into(),
        link: link::CoopLink::new(Box::new(link::HleTransport::new(live))),
        loaded: None,
        opening: None,
        editor: None,
        draft: None,
        marks: Arc::default(),
        marks_at: None,
        marks_preview: false,
        bodies: Vec::new(),
        reason: "Waiting for a local multiplayer map.".into(),
        reloader: None,
        carry: None,
    }));
    if registry
        .add(&Hooks(driver.clone()))
        .map_err(anyhow::Error::msg)?
    {
        controls::publish(controls::State::default());
        crate::forge_screens::install(registry)?;
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&driver);
        if let Some(a) = agent {
            // Rebuild map and the reload after an AI ground build run the
            // agent's map reload (`reload`).
            if let Some(sim) = &a.sim {
                let reloader: Box<dyn reload::Reloader> = Box::new(sim.map_reloader(&a.agent));
                driver.lock().unwrap_or_else(|e| e.into_inner()).reloader = Some(reloader);
                reload::set_reloads(true);
            }
            local::install(&a.agent, Backend(driver.clone()));
            agent::forge::test::install(&a.agent, Backend(driver));
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

/// `forge.waves.test`: the session's Test waves and Stop waves.
impl agent::forge::test::WavesTest for Backend {
    fn waves_test(&self, stop: bool) -> Result<agent::forge::test::TestReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.waves_test(stop)
    }
}

impl LocalApi for Backend {
    fn who(&self, m: Option<&dyn guest_layout::GuestMemory>) -> Who {
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

    fn ground_build(
        &self,
        if_stale: bool,
        classes: Option<Vec<String>>,
    ) -> Result<local::GroundReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        if let Some(classes) = classes {
            e.set_ground_classes(&classes)?;
        }
        e.ground_build(if_stale)
    }

    fn nav(&self) -> Result<Value, String> {
        self.read(Editor::nav_json)
    }

    fn set_waves(&self, set: Option<forge_doc::waves::Def>) -> Result<Value, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.set_waves(set)
    }

    fn nav_generate(&self) -> Result<local::NavReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.nav_generate()
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

    fn input(&self, m: &dyn guest_layout::GuestMemory, input: forge::tool::Input) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use forge::look::body::{Draw, Light};

    use super::reload::{Carry, PlayCopy, Step};

    fn driver() -> super::Driver {
        super::Driver {
            looks: forge::look::monitors::Looks::new(Vec::new(), None),
            store: None,
            calls: agent::sim::calls::shared(),
            vfs: None,
            data: ".".into(),
            link: super::link::CoopLink::new(Box::new(super::link::HleTransport::new(None))),
            loaded: None,
            opening: None,
            editor: None,
            draft: None,
            marks: Default::default(),
            marks_at: None,
            marks_preview: false,
            bodies: Vec::new(),
            reason: String::new(),
            reloader: None,
            carry: None,
        }
    }

    fn copy(e: &super::Editor) -> PlayCopy {
        PlayCopy {
            file: "/tmp/testroom.map".into(),
            built: e.session.rebuild_form().unwrap(),
            kind: forge::CopyKind::Rebuild,
        }
    }

    /// A change of the map's scenario or checksum resets the driver. A
    /// reload of the session's own carries the editor across it, for the
    /// session that opens on the copy it queued; any other ends it.
    #[test]
    fn a_map_change_after_a_rebuild_keeps_the_editor_for_the_copy() {
        let mut d = driver();
        let e = super::editing::test_editor();
        let copy = copy(&e);
        d.editor = Some(e);
        d.carry = Some(Carry::new(copy, super::reload::tests::answer(Some(Ok(9)))));
        assert_eq!(d.carry.as_mut().unwrap().poll(), Step::Queued(9));
        d.reset();
        assert!(d.editor.is_none());
        let carry = d.carry.as_ref().unwrap();
        assert!(carry.editor.is_some(), "the editor goes across the reload");
        assert!(carry.opens(9));
        assert!(!carry.opens(10), "another map is not the copy");
        // A second reset (the game's start after the transition) keeps it.
        d.reset();
        assert!(d.carry.as_ref().unwrap().opens(9));

        // Without a reload of ours, the change ends the session.
        let mut d = driver();
        d.editor = Some(super::editing::test_editor());
        d.reset();
        assert!(d.editor.is_none() && d.carry.is_none());
    }

    fn draw(seat: u8, model: Option<u32>, light: Option<Light>) -> Draw {
        Draw {
            seat,
            model,
            frame: forge::Frame {
                position: [1.0, 2.0, 3.0 + f32::from(seat)],
                rotation: [0.0; 3],
            },
            sphere: ([1.0, 2.0, 3.0], 1.0),
            primary: 0,
            secondary: 0,
            highlight: [0; 4],
            light,
        }
    }

    /// ML5: a body with a light carries one host light at its origin, in
    /// the palette colour, with no datum and no shadow; a marker, a body
    /// without a light and a colour the map lacks carry none.
    #[test]
    fn a_drawn_body_with_a_light_carries_one_host_light_in_its_palette_colour() {
        let lit = Light { colour: 4, radius: 4.0 };
        let draws = [
            draw(1, Some(0), Some(lit)),
            draw(2, None, Some(lit)),
            draw(3, Some(0), None),
            draw(5, Some(1), Some(Light { colour: 17, radius: 2.0 })),
        ];
        let rgb = |index: u8| (index == 4).then_some([255, 0, 51]);
        let lights = super::host_lights(&draws, rgb);
        assert_eq!(lights.len(), 1);
        let l = &lights[0];
        let none = frame_scene::NO_DATUM;
        assert_eq!((l.host, l.handle, l.definition), (Some(1), none, none));
        assert!(!l.shadowed() && !l.guest_selected);
        let c = l.candidate.as_ref().unwrap();
        assert_eq!(c.position, [1.0, 2.0, 4.0]);
        assert_eq!(c.colour_a, [1.0, 0.0, 0.2]);
        assert_eq!(c.bounds.1, 4.0);
    }
}
