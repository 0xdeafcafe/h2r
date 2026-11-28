//! Translates one local input stream into tool commands. A staged session
//! becomes visible only when the queued game call has returned its handle.
//! Its screens are the game's (`crate::forge_screens`): it builds their
//! sheets and acts on their choices.
use super::{GENERATION, admitted, controls};
use agent::forge::ground as ground_api;
use agent::forge::nav as nav_api;
use agent::forge::local::{self as local_api, ForgeEdit, ForgeWorld, Guard, ProposalReply};
use agent::sim::calls;
use forge::camera::Camera;
use forge::tool::{Action, Axes, Command, Extent, Input, Overlay, Show, Tool};
use forge::files::variants::{self, Listed};
use forge::{BlockKind, Linkable, OpId, PaletteItem, Session, Thing, ThingKind};
use forge_doc::{ActiveLink, Endpoint, Entry, Link as DocLink, LogHead, Op, Sha256};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
};

use super::reload::{self, PlayCopy};
use crate::firefight::feature::WaveTests;
use crate::firefight::runtime::{TEST_RUNNING, TestEvent, TestPhase, WAVES_STOPPED};
use crate::forge_map::Opened;
use crate::forge_screens::{
    self as screens, Facts,
    sheet::{self, ANGLE_STEPS, Ask, Choice, Group, HEADINGS, Kind, Prop, Sheet, Tool as ToolKind},
    variants as flows,
};

mod area;
mod body;
mod net;
mod resize;

pub(crate) use net::MonitorStart;

enum After {
    Tool(Command),
    Area,
    /// Items of a shared session's stream.
    Shared(Vec<forge::net::Stamped>),
    Load(String),
    Proposal(mpsc::Sender<Result<Entry, String>>),
    /// An edit that leaves the tool's state as it is.
    Edited,
    /// The world brought to the document after a reload.
    Reopened,
}

struct Pending {
    edit: ForgeEdit<()>,
    after: After,
}

enum FileResult {
    /// The map's variants, for Load, the saved prefabs, and a line for each
    /// prefab file that did not read.
    Listed(Vec<Listed>, Vec<forge::Prefab>, Vec<String>),
    /// A prefab was saved as this name; the prefabs after, and the files
    /// that did not read.
    Prefab(String, Vec<forge::Prefab>, Vec<String>),
    /// Saved as this variant name; the map's variants after.
    Saved(String, Vec<Listed>),
    /// The store has a variant of this name: Replace asks.
    Taken(String),
    /// A map variant was renamed, copied or deleted: the notice, the map's
    /// variants after, and a rename's old and new names.
    Variants(String, Vec<Listed>, Option<(String, String)>),
    Loaded(Pending),
    /// The ground build's copy of the session, whether it built, and the
    /// map it made, written for the reload.
    Ground(Box<(Session, bool, Option<PlayCopy>)>),
    /// **Rebuild map**'s copy, written for the reload.
    Rebuilt(Box<PlayCopy>),
    /// The nav generation's copy of the session.
    Nav(Box<Session>),
}

/// Where a ground build's answer goes when an agent asked for it.
type GroundAnswer = mpsc::Sender<Result<Value, String>>;

/// Where a nav generation's answer goes when an agent asked for it.
type NavAnswer = mpsc::Sender<Result<Value, String>>;

/// Where a wave test's answer goes when an agent asked for it
/// (`forge.waves.test`).
type TestAnswer = mpsc::Sender<Result<Value, String>>;

#[derive(Clone, Copy)]
enum CoordinateTarget {
    New(usize),
    Existing(Endpoint),
}

/// How far under a carried thing the readout looks for the structure.
const GROUND_REACH: f32 = 100.;
/// How far in front of the monitor a ground marker is placed, in world
/// units.
const MARKER_REACH: f32 = 3.;
/// How far from an object the placed objects browser's fly-to stands, in
/// world units.
const FLY_DISTANCE: f32 = 5.;

pub(super) struct Editor {
    pub session: Arc<Session>,
    tags: Arc<[u32]>,
    walls: Vec<bots::los::Walls>,
    bsp: Option<usize>,
    guard: Guard,
    calls: calls::Shared,
    tool: Tool,
    pending: Option<Pending>,
    file_job: Option<mpsc::Receiver<Result<FileResult, String>>>,
    data: PathBuf,
    /// The name of the map variant the session was last saved or loaded
    /// as; none for a session opened from Original.
    filename: Option<String>,
    selected: usize,
    rotation_step: usize,
    movement_step: usize,
    /// The height of the carried thing above the structure under it, from
    /// the last tick's downward ray: the readout's hold line.
    hold_height: Option<f32>,
    local_axes: bool,
    /// The Coordinates target and its current frame, while its screen is up.
    coordinates: Option<(CoordinateTarget, forge::Frame)>,
    area: Option<area::Draft>,
    /// Dev view is on (`forge.blockout.dev_view`, docs/game/forge-blockout.md).
    dev_view: bool,
    /// The map's surfaces the Material picker lists after Dev, as shader
    /// paths (`content_kit::blockout::palette::survey`); empty until a survey is read.
    surfaces: Vec<String>,
    /// Settings the player changed on a screen, for the driver to write to
    /// the profile's store ([`Editor::take_setting_writes`]).
    setting_writes: Vec<(&'static str, settings::Value)>,
    notice: Option<(String, u32)>,
    /// The right trigger is held: a held thing is being turned.
    rotating: bool,
    /// The sentence of the last refused edit and its tick: the readout's
    /// guide row says it for [`crate::forge_readout::REFUSAL_TICKS`].
    refusal: Option<(String, u32)>,
    /// The link the Link dialog shows.
    link_dialog: Option<OpId>,
    /// The delete or removal the question is about.
    delete_asked: Option<Command>,
    /// The own thing a Restore original question is about.
    restore_asked: Option<Endpoint>,
    /// The name the overwrite question is about.
    overwrite_asked: Option<String>,
    /// The variant file the load question is about.
    load_asked: Option<String>,
    /// The variant file the delete question is about.
    variant_asked: Option<String>,
    /// A question that could not open because a Forge screen was still
    /// closing: tried again each tick.
    ask_wait: Option<(Ask, String)>,
    /// The log head as last saved or loaded: edits since are not saved.
    saved_head: LogHead,
    /// The map's variants, for Load.
    variants: Vec<Listed>,
    /// The saved prefabs, which the object menu lists when the map holds
    /// their tags.
    prefabs: Vec<forge::Prefab>,
    /// They were asked for, at the first tick.
    listed: bool,
    /// A screen is up, ours or the game's.
    ui_up: bool,
    /// The map's palette entries the Objects palette leaves out, with why.
    refused: Vec<String>,
    /// The map's item collections that hold a weapon (`forge_map::Opened::armed`).
    armed: Arc<std::collections::BTreeSet<String>>,
    /// Set in a shared session (docs/game/forge-design.md, F5).
    net: Option<net::Net>,
    /// A film's edit records handed over and not yet applied, in film
    /// order ([`Editor::film_tick`]).
    played: Vec<Vec<u8>>,
    /// The tick the last [`Editor::tick`] ran at, for a notice asked for
    /// between ticks.
    now: u32,
    /// The file job is a ground build (`forge_glue::ground`).
    ground_building: bool,
    /// The agent that asked for the running build, to answer when it ends.
    ground_answer: Option<GroundAnswer>,
    /// The file job is a nav generation (`forge_glue::nav`).
    nav_generating: bool,
    /// The log head the staged nav mesh was generated at: the bots take
    /// the mesh only while the document is still this one.
    nav_staged: Option<LogHead>,
    /// The built ground's sectors as overlay lines, drawn in the AI tool
    /// while the ground is current ([`Editor::nav_lines`]).
    sector_lines: Vec<forge::NavLine>,
    /// The log head `sector_current` was read at.
    sector_head: Option<LogHead>,
    /// The ground the sectors came from still covers the document.
    sector_current: bool,
    /// The agent that asked for the running generation.
    nav_answer: Option<NavAnswer>,
    /// The running cache's name, which a play copy's file is named for.
    map_name: String,
    /// A play copy written and waiting for the driver to reload onto it
    /// ([`Editor::take_copy`]).
    copy: Option<PlayCopy>,
    /// The driver is reloading the map onto a copy: edits wait.
    reloading: bool,
    /// The Firefight runtime **Test waves** runs the session's set on
    /// (`crate::firefight::feature::wave_tests`); none without the feature.
    wave_tests: Option<Arc<dyn WaveTests>>,
    /// Test waves built the ground: the test starts once the session goes
    /// on over the map reloaded onto it.
    test_after_build: bool,
    /// The agent that asked for the test, answered when it starts.
    test_answer: Option<TestAnswer>,
    /// The set the test plays: its name and `Def::id`.
    test_set: Option<(String, Option<String>)>,
    /// What the Waves page's Start from offers on this base map
    /// (`forge_screens::waves::presets`), read once as the session opens.
    wave_presets: Arc<[forge_doc::waves::Def]>,
    /// The Forge bodies the map's kit carries (`forge::body::Kit`).
    bodies: forge::body::Kit,
    /// The body D-pad Up swaps into: the profile's choice ([`Editor::set_body`]).
    body: forge_doc::bodies::Body,
    /// Where the local player's unit is in the swap ([`body`]).
    swap: body::Swap,
    /// Swap into the body as soon as the player has a unit: a Forge
    /// lobby's start, and a reload of ours from inside the body.
    enter_at_start: bool,
    /// The local player's handle, read each tick.
    player: Option<u32>,
    /// The world point the object actions menu is anchored at, while it
    /// is asked for or up (`forge_screens::object_menu`).
    menu_at: Option<[f32; 3]>,
    /// The guest's horizontal field of view at 4:3, radians, as last read
    /// ([`FOV_GLOBAL`]); [`MENU_FOV`] until then.
    menu_fov: f32,
}

/// The guest's horizontal field of view, radians (`crate::render_aspect`
/// reads the same word for the ocean's grid).
const FOV_GLOBAL: u32 = 0x0048_5640;
/// The lens taken until the guest's is read: 70 degrees across.
const MENU_FOV: f32 = 1.221_730_5;

impl Editor {
    pub fn new(opened: Opened, data: &Path, scenario: u32, calls: calls::Shared) -> Self {
        let saved_head = opened.session.head();
        let map_name = opened.session.document().title.clone();
        let wave_presets = screens::waves::presets(&opened.session.document().base.map);
        let tool = Tool::with_extents(opened.extents.into());
        let enter_at_start = crate::lobby_start::mode() == crate::lobby_start::Mode::Forge;
        Self {
            session: Arc::new(opened.session),
            tags: opened.tags.into(),
            walls: opened.walls,
            bsp: None,
            guard: Guard {
                scenario,
                generation: GENERATION.load(Ordering::Acquire),
                admitted,
            },
            calls,
            tool,
            pending: None,
            file_job: None,
            data: data.into(),
            filename: None,
            selected: 0,
            rotation_step: 2,
            movement_step: 0,
            hold_height: None,
            local_axes: false,
            coordinates: None,
            area: None,
            dev_view: false,
            surfaces: Vec::new(),
            setting_writes: Vec::new(),
            notice: None,
            rotating: false,
            refusal: None,
            link_dialog: None,
            delete_asked: None,
            restore_asked: None,
            overwrite_asked: None,
            load_asked: None,
            variant_asked: None,
            ask_wait: None,
            saved_head,
            variants: Vec::new(),
            prefabs: Vec::new(),
            listed: false,
            ui_up: false,
            refused: opened.refused,
            armed: Arc::new(opened.armed),
            net: None,
            played: Vec::new(),
            now: 0,
            ground_building: false,
            ground_answer: None,
            nav_generating: false,
            nav_staged: None,
            sector_lines: Vec::new(),
            sector_head: None,
            sector_current: false,
            nav_answer: None,
            map_name,
            copy: None,
            reloading: false,
            wave_tests: crate::firefight::feature::wave_tests(),
            test_after_build: false,
            test_answer: None,
            test_set: None,
            wave_presets,
            bodies: opened.bodies,
            body: forge_doc::bodies::Body::Monitor,
            swap: body::Swap::Biped,
            enter_at_start,
            player: None,
            menu_at: None,
            menu_fov: MENU_FOV,
        }
    }

    /// The body D-pad Up swaps into from now on: the profile's
    /// (`forge::body::load`).
    pub fn set_body(&mut self, body: forge_doc::bodies::Body) {
        self.body = body;
    }

    /// The body the player flies and its unit, for `forge.status`.
    #[cfg(test)]
    pub fn flying(&self) -> Option<(forge_doc::bodies::Body, u32)> {
        self.swap.flying()
    }

    /// The body picker's rows on this map (`forge::body::rows`).
    pub fn body_rows(&self) -> Vec<forge::body::Row> {
        forge::body::rows(&self.bodies)
    }

    /// The running cache's name (`forge_map::Loaded::name`).
    pub fn set_map_name(&mut self, name: String) {
        self.map_name = name;
    }

    /// A play copy waiting for the reload, once: the driver reloads onto
    /// it and carries the editor across ([`super::reload::Carry`]).
    pub fn take_copy(&mut self) -> Option<PlayCopy> {
        self.copy.take()
    }

    /// The driver reloads the map (edits wait), or has stopped. A reload
    /// that stopped without going on over the copy leaves a Test waves
    /// that waited for it untested.
    pub fn set_reloading(&mut self, reloading: bool) {
        self.reloading = reloading;
        if !reloading && std::mem::take(&mut self.test_after_build) {
            self.answer_test(Err(super::ground::NOT_LOADED.into()));
        }
    }

    /// The session ends (the driver leaves the map): a wave test stops
    /// first.
    pub fn leave(&mut self) {
        self.stop_test(self.now);
    }

    /// **Rebuild map**: build the map from the document on a worker, write
    /// it as the play copy and hand it to the driver to reload onto
    /// ([`Editor::take_copy`]). A session whose map has every edit does
    /// nothing; a held thing is dropped first.
    fn start_rebuild(&mut self, tick: u32) -> Result<(), String> {
        if self.is_shared() {
            return Err("Rebuild map is for a local session.".into());
        }
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        if self.tool.held_thing.is_some() {
            return Err("Drop what the monitor holds first.".into());
        }
        if !self.session.rebuild_needed() {
            self.notice = Some((reload::NOTHING.into(), tick));
            return Ok(());
        }
        if !reload::reloads() {
            return Err(reload::NO_RELOAD.into());
        }
        if !reload::can_build() {
            return Err(reload::NO_BUILDER.into());
        }
        let built = self.session.rebuild_form()?;
        // The rebuilt map has none of the test's actors.
        self.stop_test(tick);
        let file = reload::copy_path(&self.data, &self.map_name);
        self.file(move || {
            let bytes = reload::build(&built)?;
            reload::write_copy(&file, &bytes)?;
            Ok(FileResult::Rebuilt(Box::new(PlayCopy {
                file,
                built,
                kind: forge::CopyKind::Rebuild,
            })))
        })?;
        self.notice = Some((reload::REBUILDING.into(), tick));
        Ok(())
    }

    /// Go on over the map a reload of ours loaded (`opened`, the session
    /// opened on it at `scenario`): the document, the log, undo and redo,
    /// the monitor's place and the tool are kept, and the copy's rows are
    /// what the world holds ([`forge::Session::adopt_copy`]). `live` finds
    /// an added object's live object by its block and row. The selection
    /// and any hold are let go. The world is then brought to the document
    /// as after an edit. An `Err` is why the session cannot go on over it,
    /// with `opened` back for a fresh session.
    #[allow(clippy::result_large_err)] // `Opened` goes back by value, once per reload.
    pub fn reopen(
        mut self,
        opened: Opened,
        scenario: u32,
        copy: &PlayCopy,
        mut live: impl FnMut(BlockKind, u32) -> Option<u32>,
        tick: u32,
    ) -> Result<Self, (String, Opened)> {
        if opened.session.palette() != self.session.palette() {
            return Err(("The rebuilt map's palette is not the session's.".into(), opened));
        }
        let baked = match self.session.baked_objects(&copy.built) {
            Ok(baked) => baked,
            Err(why) => return Err((why, opened)),
        };
        let handles = baked
            .into_iter()
            .filter_map(|(op, block, index)| Some((op, live(block, index)?)))
            .collect();
        let found = forge::Reopened {
            handles,
            room: opened.session.kit_room(),
        };
        let mut session = Session::clone(&self.session);
        if let Err(why) = session.adopt_copy(&copy.built, copy.kind, found) {
            return Err((why, opened));
        }
        self.session = Arc::new(session);
        self.tags = opened.tags.into();
        self.bodies = opened.bodies;
        // The reload made a new world with no body in it: the player is
        // back in a biped, and goes back into the body once spawned.
        if self.tool.monitor || self.swap.flying().is_some() {
            self.enter_at_start = true;
        }
        self.swap = body::Swap::Biped;
        self.tool.monitor = false;
        self.tool.camera = None;
        self.walls = opened.walls;
        self.refused = opened.refused;
        self.armed = Arc::new(opened.armed);
        self.bsp = None;
        self.guard = Guard {
            scenario,
            generation: GENERATION.load(Ordering::Acquire),
            admitted,
        };
        self.pending = None;
        self.file_job = None;
        self.copy = None;
        self.reloading = false;
        self.area = None;
        self.coordinates = None;
        self.hold_height = None;
        self.tool.clear_selection();
        self.notice = Some((copy.done().into(), tick));
        // The session holds what the copy holds; a refused call is said,
        // and the next edit brings the world to the document again.
        if let Err(why) = self.stage(After::Reopened, |s, w| s.release(w)) {
            self.refuse(why, tick);
        }
        // Test waves built this map's ground: the test starts on it.
        if std::mem::take(&mut self.test_after_build) {
            let started = if self.session.ground_loaded() {
                self.begin_test(None)
            } else {
                Err(super::ground::NOT_LOADED.into())
            };
            if let Err(why) = started {
                self.refuse(why, tick);
            }
        }
        Ok(self)
    }

    /// Loads the saved map `name` into the session as it opens (the Forge
    /// lobby's choice). A refusal is the editor's notice.
    pub fn load_draft(&mut self, tick: u32, name: String) {
        if let Err(reason) = self.choose(tick, Choice::Load(name)) {
            self.notice = Some((reason, tick));
        }
    }

    /// Whether an overlay is on.
    pub fn overlay(&self, overlay: Overlay) -> bool {
        self.tool.overlays.on(overlay)
    }

    pub fn monitor(&self) -> bool {
        self.tool.monitor
    }

    /// The object actions menu's object in the window now, in normalised
    /// device coordinates (-1 to 1, Y up): its point through the monitor
    /// camera, by the guest's lens at 4:3 with the height kept on a wider
    /// window. `None` behind the camera, or with no menu or no monitor.
    fn menu_point(&self) -> Option<[f32; 2]> {
        let (at, camera) = (self.menu_at?, self.tool.camera?);
        let (w, h) = crate::render_aspect::layout_output();
        let [forward, _, up] = camera.basis();
        crate::ui_kit::floating::project(
            &crate::ui_kit::floating::Camera {
                position: camera.position,
                forward,
                up,
                fov_y: 2.0 * ((self.menu_fov * 0.5).tan() * 0.75).atan(),
                aspect: w.max(1) as f32 / h.max(1) as f32,
            },
            at,
        )
    }

    /// The monitor camera's position, yaw and pitch, for
    /// `forge.status.monitors`.
    pub fn pose(&self) -> Option<([f32; 3], f32, f32)> {
        self.tool.camera.map(|c| (c.position, c.yaw, c.pitch))
    }

    /// An edit was refused: the sentence is the notice and the readout's
    /// guide row.
    fn refuse(&mut self, reason: String, tick: u32) {
        self.refusal = Some((reason.clone(), tick));
        self.notice = Some((reason, tick));
    }

    /// Says `text` in the HUD strip for a while, as a file notice is.
    pub fn say(&mut self, text: String, tick: u32) {
        self.notice = Some((text, tick));
    }

    /// Whether the gizmo uses the object's own axes at pickup.
    pub fn local_axes(&self) -> bool {
        self.tool.axes == Axes::Local
    }

    /// The proposed frame of what is placed or held, with its bounding
    /// sphere: the gizmo's axes and outline.
    pub fn ghost(&self) -> Option<(forge::Frame, Extent)> {
        if self.monitor() && !self.ui_up {
            Some((self.tool.ghost?, self.tool.extent(&self.session)?))
        } else {
            None
        }
    }

    /// What the Forge readout shows this tick (docs/game/
    /// forge-map-variants-ui.md, "The readout"): the thing aimed at or held,
    /// and the things with no body for the near marks. Nothing with the
    /// monitor off.
    pub fn readout_scene(&self, game: Option<i32>) -> crate::forge_readout::Scene {
        use crate::forge_readout as readout;
        let Some(camera) = self.tool.camera.as_ref().filter(|_| self.tool.monitor) else {
            return readout::Scene::default();
        };
        let things = self.session.things();
        let target = self
            .tool
            .held_thing
            .or(self.tool.aimed_thing)
            .and_then(|id| self.session.thing(id))
            .map(|thing| {
                let held = self.tool.held_thing.is_some();
                let frame = self.tool.ghost.filter(|_| held).unwrap_or(thing.frame);
                let mut aimed = readout::aimed(
                    &thing,
                    thing_name(&thing),
                    frame.position,
                    frame.rotation[0].to_degrees(),
                    self.tool.thing_extent(&self.session, &thing).radius,
                    game,
                );
                if let Some((name, note)) = self.marker_words(thing.id) {
                    aimed.target.name = name;
                    aimed.target.thing =
                        hud_kit_core::views::forge_readout::Thing::Marker { note };
                }
                aimed
            });
        // An added thing is what a hold spends the budget on; moving an own
        // entry costs nothing.
        let budget = self
            .tool
            .held_thing
            .and_then(|id| self.session.thing(id))
            .filter(|thing| !thing.own)
            .and_then(|thing| self.session.budget_for(thing.block))
            .and_then(|b| crate::forge_readout::budget_of(&b));
        let zone = self
            .tool
            .held_thing
            .or(self.tool.aimed_thing)
            .and_then(|id| self.session.thing(id))
            .map(|goal| readout::zone_of(&goal, &things))
            .unwrap_or_default();
        // The hold line needs the ground under the thing: with no structure
        // within reach it is absent.
        let hold = self
            .tool
            .held_thing
            .zip(self.hold_height)
            .zip(target.as_ref())
            .map(|((_, height), aimed)| {
                let snap = sheet::MOVE_STEPS[self.movement_step];
                forge_readout_hold(
                    aimed.at,
                    height,
                    (snap > 0.).then_some(snap),
                    self.local_axes(),
                    self.tool.picked_up_at(),
                )
            });
        readout::Scene {
            monitor: true,
            eye: camera.position,
            hold,
            zone,
            target,
            refusal: self.refusal.as_ref().map(|(text, _)| text.clone()),
            budget,
            things: things
                .iter()
                .filter_map(|t| readout::placed(t, game))
                .collect(),
        }
    }

    pub fn status(&self) -> Value {
        self.shared_status(self.local_status())
    }

    fn local_status(&self) -> Value {
        let screen = screens::shown().map(|s| {
            json!({
                "name":s.kind.screen(), "title":s.title, "focus":(!s.rows.is_empty()).then_some(s.focus),
                "rows":s.rows.iter().map(|r| &r.label).collect::<Vec<_>>(),
            })
        });
        json!({
            "session":"local", "role":"editor", "monitor":self.tool.monitor,
            "camera":self.tool.camera.map(|c| json!({"position":c.position,"yaw":c.yaw,"pitch":c.pitch})),
            "aimed":self.tool.aimed, "held":self.tool.held, "placing":self.tool.placing,
            "aimed_thing":self.tool.aimed_thing, "held_thing":self.tool.held_thing,
            "aimed_drop":self.aimed_drop().map(|(zone, squads)| json!({"zone":zone,"squads":squads})),
            "show":self.tool.show.name(), "game":self.tool.game,
            "area_edit":self.area.as_ref().map(area::Draft::status),
            "variant":local_api::variant_status(self.session.document(), self.filename.as_deref(), self.dirty()),
            "screen":screen, "head":self.session.head(), "pending_rebuild":self.session.pending_rebuild(),
            "tool":if self.tool.nav { "nav" } else if self.tool.ai { "ai" } else if self.tool.links { "links" } else { "objects" }, "fine":controls::fine(),
            "rotation_snap":ANGLE_STEPS[self.rotation_step], "movement_snap":sheet::MOVE_STEPS[self.movement_step],
            "axes":self.tool.axes.name(),
            "block_grid":self.tool.block_grid, "dev_view":self.dev_view,
            "resize":self.tool.draft.as_ref().map(|d| json!({"target":d.id, "def":d.def.name(), "size":d.def.size})),
            "overlays":Value::Object(Overlay::ALL.iter()
                .map(|o| (o.name().to_string(), Value::Bool(self.tool.overlays.on(*o)))).collect()),
            "ui_up":self.ui_up, "dirty":self.dirty(),
            "aimed_link":self.tool.aimed_link.map(linkable_json), "link_from":self.tool.link_from.map(linkable_json),
            "teleporters":self.session.teleporters().iter().map(|t| json!({
                "flag":t.index, "sender":t.sender, "channel":t.channel, "position":t.position,
            })).collect::<Vec<_>>(),
            "added_teleporters":self.session.added_teleporters().iter().map(|t| json!({
                "id":t.id, "sender":t.sender, "channel":t.channel, "position":t.position,
            })).collect::<Vec<_>>(),
            "links":self.links_json(), "groups":self.groups_json(), "selection":self.tool.selection(),
            "labels":self.labels_json(), "prefabs":self.prefabs_json(),
            "budget":{"used":self.session.objects().len(),"limit":self.session.budget()},
            "palette":self.session.palette().len(), "palette_refused":self.refused,
            "ground":ground_api::status(&self.session, self.ground_facts()),
            "nav":nav_api::status(&self.session, self.nav_facts()),
            "parked":controls::parked().map(|unit| format!("{unit:#010x}")),
            "body":self.swap.flying().map(|(body, unit)| json!({"kind":body.key(), "unit":format!("{unit:#010x}")})),
            "body_choice":self.body.key(),
            "bodies":self.body_rows().iter().map(|r| json!({"kind":r.body.key(), "name":r.label, "refused":r.refused})).collect::<Vec<_>>(),
            "pending_call":self.pending.is_some(), "pending_file":self.file_job.is_some(), "file":self.filename,
            "notice":self.notice.as_ref().map(|(text,_)|text)
        })
    }

    /// What the game knows of the ground besides the session
    /// (`forge.ground`).
    fn ground_facts(&self) -> ground_api::Facts {
        ground_api::Facts {
            available: super::ground::available(),
            building: self.ground_building,
            loaded: self.session.ground_loaded(),
        }
    }

    /// `forge.ground`'s reply.
    pub fn ground_json(&self) -> Value {
        ground_api::reply(&self.session, self.ground_facts())
    }

    /// **Build AI ground** (`if_stale` false) or **Test waves**: build the
    /// ground on a worker over a copy of the session, which the editor takes
    /// back when the worker is done. Edits wait meanwhile. `answer` is told
    /// how it ended.
    fn start_ground(
        &mut self,
        tick: u32,
        if_stale: bool,
        answer: Option<GroundAnswer>,
    ) -> Result<(), String> {
        if self.is_shared() {
            return Err("AI ground is built in a local session.".into());
        }
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        if !super::ground::available() {
            return Err(super::ground::NO_BUILDER.into());
        }
        // The ground's map has none of the test's actors.
        self.stop_test(tick);
        let session = self.session.clone();
        let file = reload::reloads().then(|| reload::copy_path(&self.data, &self.map_name));
        self.file(move || {
            let mut copy = Session::clone(&session);
            // A map an earlier build left is not this one's.
            let _ = reload::take_ground_copy();
            let built = super::ground::run(&mut copy, if_stale)?;
            let play = match (built, file, reload::take_ground_copy()) {
                (true, Some(file), Some(bytes)) => {
                    reload::write_copy(&file, &bytes)?;
                    Some(PlayCopy {
                        built: copy.ground_copy_form()?,
                        file,
                        kind: forge::CopyKind::Ground,
                    })
                }
                _ => None,
            };
            Ok(FileResult::Ground(Box::new((copy, built, play))))
        })?;
        self.ground_building = true;
        self.ground_answer = answer;
        self.notice = Some(("Building AI ground...".into(), tick));
        Ok(())
    }

    /// `forge.ground.build`: start a build and answer when it has ended.
    pub fn ground_build(
        &mut self,
        if_stale: bool,
    ) -> Result<mpsc::Receiver<Result<Value, String>>, String> {
        let (tx, rx) = mpsc::channel();
        self.start_ground(self.now, if_stale, Some(tx))?;
        Ok(rx)
    }

    /// `forge.ground.build {classes}`: ask for the enemy classes past the
    /// Elite (docs/game/forge-firefight.md, "Enemy classes"). The next
    /// build chooses each donor from this machine's disc. Refused while an
    /// edit or a file job waits, and for a name the table lacks.
    pub fn set_ground_classes(&mut self, names: &[String]) -> Result<(), String> {
        if self.is_shared() {
            return Err("AI ground is built in a local session.".into());
        }
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        let mut next = Session::clone(&self.session);
        next.set_ground_classes(names)?;
        self.session = Arc::new(next);
        Ok(())
    }

    /// `forge.waves {set}`: replace or clear the session's wave set
    /// (docs/game/forge-firefight.md, "The wave set") and answer
    /// `forge.waves`' reply. Refused while an edit or a file job waits, as
    /// the session it lands would not carry the set.
    pub fn set_waves(&mut self, set: Option<forge_doc::waves::Def>) -> Result<Value, String> {
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        let mut next = Session::clone(&self.session);
        next.set_waves(set).map_err(|e| e.to_string())?;
        self.session = Arc::new(next);
        Ok(agent::forge::waves::reply(&self.session))
    }

    /// **Test waves** (docs/game/forge-firefight.md, "Test waves"): run the
    /// session's wave set on the running map, with the editor still open.
    /// With the current ground on the running map the runtime starts the
    /// test at the next tick; else the ground is built, the map reloaded
    /// onto it, and the test starts when the session goes on over it
    /// ([`Editor::reopen`]). `answer` hears when it started or was
    /// refused, or at once that the ground is building.
    fn test_waves(&mut self, tick: u32, answer: Option<TestAnswer>) -> Result<(), String> {
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        if self.testing() {
            return Err(TEST_RUNNING.into());
        }
        if self.session.ground_loaded() {
            self.test_answer = None;
            return self.begin_test(answer);
        }
        self.start_ground(tick, true, None)?;
        self.test_after_build = true;
        if let Some(answer) = answer {
            let _ = answer.send(Ok(json!({"testing": false, "building": true})));
        }
        Ok(())
    }

    /// Hand the session's set to the runtime, which starts the test at the
    /// next tick boundary.
    fn begin_test(&mut self, answer: Option<TestAnswer>) -> Result<(), String> {
        let tests = self
            .wave_tests
            .clone()
            .ok_or(super::ground::NO_FIREFIGHT)?;
        let def = super::ground::waves_or_default(&self.session);
        let set = (def.name.clone(), def.id().ok().map(|id| id.to_string()));
        let problem = screens::waves::first_problem(&def, &self.session.wave_problems());
        tests.start(def)?;
        self.test_set = Some(set);
        self.test_answer = answer;
        if let Some(problem) = problem {
            self.notice = Some((problem, self.now));
        }
        Ok(())
    }

    /// What the Waves screens read: the set the session plays and what
    /// goes with it (docs/game/forge-firefight-ui.md, "The Waves screens").
    fn wave_facts(&self) -> screens::waves::Facts {
        screens::waves::Facts {
            set: self.session.waves_or_default(),
            own: !self.session.waves_are_default(),
            edited: self.session.waves_edited(),
            classes: super::ground::wave_classes(&self.session),
            drops: self.session.wave_drops(),
            problems: self.session.wave_problems(),
            presets: self.wave_presets.clone(),
            testing: self.testing(),
        }
    }

    /// A Waves page's value: one change of the set, one `set_waves`. Start
    /// from over an edited set asks first. A refusal builds the pages
    /// again, so the value shown goes back to the set's.
    fn edit_waves(&mut self, edit: screens::waves::Edit) -> Result<(), String> {
        let facts = self.wave_facts();
        if let screens::waves::Edit::StartFrom(i) = edit
            && let screens::waves::Start::Ask(question) = screens::waves::start_from(&facts, i)?
        {
            return screens::ask_over(question);
        }
        self.apply_waves(&facts, &edit)
    }

    fn apply_waves(
        &mut self,
        facts: &screens::waves::Facts,
        edit: &screens::waves::Edit,
    ) -> Result<(), String> {
        let next = screens::waves::apply(&facts.set, edit, &facts.presets)
            .and_then(|def| self.set_waves(Some(def)));
        if next.is_err() {
            screens::waves::stale();
        }
        next.map(|_| ())
    }

    /// Yes to a Waves question: its edit, or for Reset waves the set as the
    /// session loaded it.
    fn answer_waves(&mut self, ask: screens::waves::WavesAsk) -> Result<(), String> {
        match screens::waves::answered(ask) {
            Some(edit) => {
                let facts = self.wave_facts();
                self.apply_waves(&facts, &edit)
            }
            None => {
                let loaded = self.session.waves_loaded().cloned();
                self.set_waves(loaded).map(|_| ())
            }
        }
    }

    /// **Stop waves**, and every change of map the session makes: end the
    /// test that runs or is starting, and a Test waves waiting for its
    /// ground. Whether a test was stopped.
    fn stop_test(&mut self, tick: u32) -> bool {
        self.test_after_build = false;
        let stopped = self.testing() && self.wave_tests.as_ref().is_some_and(|t| t.stop());
        if stopped {
            self.answer_test(Err(WAVES_STOPPED.into()));
        }
        self.poll_test(tick);
        stopped
    }

    /// **Stop waves**: refused with no test to stop.
    fn stop_waves(&mut self, tick: u32) -> Result<(), String> {
        if self.stop_test(tick) {
            Ok(())
        } else {
            Err("No wave test is running.".into())
        }
    }

    /// Whether a test runs or is starting: the Forge options' row reads
    /// **Stop waves**.
    fn testing(&self) -> bool {
        self.wave_tests
            .as_ref()
            .is_some_and(|t| t.phase() != TestPhase::Off)
    }

    /// The runtime's word on the test: its start answers the agent, a
    /// refusal is said, and its end is the notice ("Waves cleared." or
    /// "Waves stopped.").
    fn poll_test(&mut self, tick: u32) {
        let Some(tests) = self.wave_tests.clone() else {
            return;
        };
        while let Some(event) = tests.take_event() {
            match event {
                TestEvent::Started => {
                    let (set, id) = self.test_set.clone().unzip();
                    self.answer_test(Ok(json!({
                        "testing": true, "set": set, "id": id.flatten(),
                    })));
                }
                TestEvent::Refused(why) => {
                    self.answer_test(Err(why.clone()));
                    self.refuse(why, tick);
                }
                TestEvent::Ended(text) => self.notice = Some((text.into(), tick)),
            }
        }
    }

    fn answer_test(&mut self, result: Result<Value, String>) {
        if let Some(answer) = self.test_answer.take() {
            let _ = answer.send(result);
        }
    }

    /// `forge.waves.test`: start a test (`stop` false) and answer when it
    /// has started, or at once when the ground builds first; or stop one.
    pub fn waves_test(&mut self, stop: bool) -> Result<mpsc::Receiver<Result<Value, String>>, String> {
        let (tx, rx) = mpsc::channel();
        if stop {
            let stopped = self.stop_test(self.now);
            let _ = tx.send(Ok(json!({"testing": false, "stopped": stopped})));
        } else {
            self.test_waves(self.now, Some(tx))?;
        }
        Ok(rx)
    }

    /// What the game knows of the nav besides the session (`forge.nav`).
    fn nav_facts(&self) -> nav_api::Facts {
        nav_api::Facts {
            available: super::nav::available(),
            generating: self.nav_generating,
        }
    }

    /// `forge.nav`'s reply.
    pub fn nav_json(&self) -> Value {
        nav_api::reply(&self.session, self.nav_facts())
    }

    /// **Generate nav**: generate on a worker over a copy of the session,
    /// which the editor takes back when the worker is done. Edits wait
    /// meanwhile. `answer` is told how it ended.
    fn start_nav(&mut self, tick: u32, answer: Option<NavAnswer>) -> Result<(), String> {
        if self.is_shared() {
            return Err("Nav is generated in a local session.".into());
        }
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        if !super::nav::available() {
            return Err(super::nav::NO_GENERATOR.into());
        }
        let session = self.session.clone();
        self.file(move || {
            let mut copy = Session::clone(&session);
            super::nav::run(&mut copy)?;
            Ok(FileResult::Nav(Box::new(copy)))
        })?;
        self.nav_generating = true;
        self.nav_answer = answer;
        self.notice = Some(("Generating nav...".into(), tick));
        Ok(())
    }

    /// `forge.nav {generate: true}`: start a generation and answer when it
    /// has ended.
    pub fn nav_generate(&mut self) -> Result<mpsc::Receiver<Result<Value, String>>, String> {
        let (tx, rx) = mpsc::channel();
        self.start_nav(self.now, Some(tx))?;
        Ok(rx)
    }

    /// The staged nav mesh goes when the document has moved on from it.
    fn retire_nav(&mut self) {
        if self.nav_staged.is_some_and(|head| head != self.session.head()) {
            self.nav_staged = None;
            super::nav::unstage();
        }
    }

    /// Whether the built ground's sectors still cover the document, read
    /// when the log head moves (the stale rule hashes the document).
    fn refresh_sectors(&mut self) {
        if self.session.ground_report().is_none() {
            self.sector_lines.clear();
            self.sector_head = None;
            self.sector_current = false;
            return;
        }
        let head = self.session.head();
        if self.sector_head != Some(head) {
            self.sector_head = Some(head);
            self.sector_current = self.session.ground_state() == forge::GroundState::Current;
        }
    }

    /// The lines the window draws: in the AI tool, the built ground's
    /// sectors while it is current; in the Nav tool with the Nav overlay
    /// on, the sectors and links of the last **Generate nav**
    /// ([`super::nav_world`]). None otherwise.
    pub fn nav_lines(&self) -> Option<&[forge::NavLine]> {
        if self.tool.monitor
            && self.tool.ai
            && self.overlay(Overlay::Markers)
            && self.sector_current
            && !self.sector_lines.is_empty()
        {
            return Some(&self.sector_lines);
        }
        if !(self.tool.monitor && self.tool.nav && self.overlay(Overlay::Nav)) {
            return None;
        }
        self.session.nav_report().map(|r| r.lines.as_slice())
    }

    /// The game type markers of the session: a marker for each spawn,
    /// flag, hill, teleporter, item spawner and ground label, and each
    /// problem the last ground build found
    /// ([`super::markers_world::entries`]), then with the Nav overlay on
    /// each problem the last **Generate nav** found
    /// ([`super::markers_world::nav_problems`]). None with the monitor off.
    pub fn marker_entries(&self) -> Vec<super::markers_world::Entry> {
        use super::markers_world::{Facts, entries, nav_problems};
        let nav = match self.session.nav_report() {
            Some(report) if self.tool.monitor && self.overlay(Overlay::Nav) => {
                nav_problems(&report.problems)
            }
            _ => Vec::new(),
        };
        if !self.tool.monitor || !self.overlay(Overlay::Markers) {
            return nav;
        }
        let placed = self.session.ground_counts().iter().sum::<usize>() > 0;
        let mut ground = if placed { self.session.markers().unwrap_or_default() } else { Default::default() };
        let mut things = self.session.things();
        if let Some((id, frame)) = self.tool.held_thing.zip(self.tool.ghost) {
            if let Some(t) = things.iter_mut().find(|t| t.id.identity() == id.identity()) {
                t.frame = frame;
            }
            if let Endpoint::Added(id) = id
                && let Some(m) = ground.iter_mut().find(|m| m.id == id)
            {
                m.frame = frame;
            }
        }
        let problems = self
            .session
            .ground_report()
            .map_or(&[][..], |r| r.problems.as_slice());
        let mut entries = entries(&Facts {
            monitor: true,
            things: &things,
            ground: &ground,
            labels: &self.session.labels().unwrap_or_default(),
            areas: &self.session.areas().unwrap_or_default(),
            problems,
            game: self.tool.game,
            show: self.tool.show,
        });
        if let Some(draft) = &self.area
            && let Some(entry) = entries.iter_mut().find(|e| e.id == Some(draft.id))
        {
            entry.marker.kind = entry.marker.kind.clone().with_area(draft.area.clone());
            entry.draft_path = matches!(draft.area.shape, game_markers::Shape::Path(_));
            // A box in hand turns with its marker, as it will once saved.
            entry.place = super::markers_world::place_of(&entry.marker.kind, entry.place.at, draft.yaw);
        }
        entries.extend(nav);
        entries
    }

    pub fn marker_preview(&self) -> bool {
        self.area.is_some()
            || self.tool.held_thing.is_some()
            || self.tool.draft.is_some()
            || self.tool.placing_piece.is_some()
    }

    /// The things aimed at and held, which their markers draw selected.
    pub fn marker_pins(&self) -> (Option<Endpoint>, Option<Endpoint>) {
        (
            self.tool.aimed_thing,
            self.area.as_ref().map(|area| area.id).or(self.tool.held_thing),
        )
    }

    /// What is placed, held or aimed at: its name, block and frame (the
    /// ghost's while it is carried).
    fn target(&self) -> Option<(String, BlockKind, forge::Frame)> {
        if let Some(draft) = &self.tool.draft {
            return Some((self.resize_line(draft), BlockKind::Crates, draft.frame));
        }
        if let Some(def) = &self.tool.placing_piece {
            return Some((
                screens::blockout::name(def),
                BlockKind::Crates,
                self.tool.ghost.unwrap_or_default(),
            ));
        }
        if let Some(index) = self.tool.placing {
            let item = self.session.palette().get(index)?;
            return Some((label(item), item.block, self.tool.ghost.unwrap_or_default()));
        }
        let id = self.tool.held_thing.or(self.tool.aimed_thing)?;
        let thing = self.session.thing(id)?;
        let name = match self.session.piece(id) {
            Some(piece) => screens::blockout::name(&piece.def),
            None => self
                .marker_words(id)
                .map_or_else(|| thing_name(&thing), |(name, _)| name),
        };
        Some((name, thing.block, self.tool.ghost.unwrap_or(thing.frame)))
    }

    /// What the AI tool calls the ground marker `id`, and the readout's
    /// fact for it: "Drop zone 3" with "2 squads land here", "Red base",
    /// "Cover". None outside the AI tool and for a thing that is not a
    /// marker. A drop zone's number is its zone in the wave set, from 0 as
    /// the Drop picker numbers it.
    fn marker_words(&self, id: Endpoint) -> Option<(String, String)> {
        if !self.tool.ai {
            return None;
        }
        let Endpoint::Added(op) = id.identity() else {
            return None;
        };
        let marker = self.session.markers().ok()?.into_iter().find(|m| m.id == op)?;
        Some(match marker.label {
            forge::GroundLabel::Drop => {
                let zone = self.session.drop_zone(op)?;
                (
                    format!("Drop zone {zone}"),
                    screens::waves::lands_here(self.session.zone_squads(zone)),
                )
            }
            forge::GroundLabel::Base => {
                let team = ["Red", "Blue"].get(usize::from(marker.team)).copied();
                (format!("{} base", team.unwrap_or("Red")), String::new())
            }
            forge::GroundLabel::Cover => ("Cover".into(), String::new()),
            other => (other.title().to_string(), String::new()),
        })
    }

    /// The zone and squad count of the drop zone the AI tool aims at.
    fn aimed_drop(&self) -> Option<(usize, usize)> {
        if !self.tool.ai {
            return None;
        }
        let Endpoint::Added(op) = self.tool.aimed_thing?.identity() else {
            return None;
        };
        let zone = self.session.drop_zone(op)?;
        Some((zone, self.session.zone_squads(zone)))
    }

    /// What the Properties page says of selecting and grouping `id`.
    fn group_facts(&self, id: Endpoint) -> sheet::GroupFacts {
        sheet::GroupFacts {
            thing: id.identity(),
            selected: self.tool.is_selected(id),
            selection: self.tool.selection().len(),
            group: self.session.group_of(id),
            pivot_editable: self.session.seat().is_none(),
        }
    }

    /// What the Properties page reads of the session for `id`: its label,
    /// and how many objects Save as prefab would take.
    fn property_extras(&self, id: Endpoint) -> sheet::Extras {
        sheet::Extras {
            labellable: true,
            label: self.session.label_of(id).map(|l| (l.text, l.number)),
            savable: self.prefab_members(id).len(),
            kit: self.session.has_kit(),
            team: self.session.team(id),
            spare_clips: self.spare_clips_row(id),
            weight: self
                .session
                .thing(id)
                .filter(|t| t.kind == forge::ThingKind::StartingPoint)
                .map(|_| self.session.spawn_weight(id)),
            place_at_start: self
                .session
                .thing(id)
                .filter(|t| t.kind == forge::ThingKind::ItemSpawner)
                .map(|_| self.session.place_at_start(id)),
        }
    }

    /// The spare clips row's value for `id`: an item spawner of an item
    /// collection (`itmc`) that holds a weapon has the row, a mixed one
    /// too; a collection of equipment, a vehicle's or any other thing's has
    /// none (`forge_map::holds_weapon`).
    pub(crate) fn spare_clips_row(&self, id: Endpoint) -> Option<Option<u8>> {
        let thing = self.session.thing(id)?;
        let armed = thing.kind == forge::ThingKind::ItemSpawner
            && thing.fields.iter().any(|(name, value)| {
                *name == "item"
                    && matches!(value, forge_doc::Value::Tag(Some(key))
                        if key.group == "itmc" && self.armed.contains(&key.path))
            });
        armed.then(|| self.session.spare_clips(id))
    }

    /// The things Save as prefab takes for `id`: its group, else the
    /// selection when it is in it with another, else itself. Nothing when
    /// one is not an object the session placed.
    fn prefab_members(&self, id: Endpoint) -> Vec<Endpoint> {
        let members: Vec<Endpoint> = match self.session.group_of(id) {
            Some(group) => group.members,
            None if self.tool.selection().len() >= 2 && self.tool.is_selected(id) => {
                self.tool.selection().to_vec()
            }
            None => vec![id],
        };
        if members
            .iter()
            .all(|m| matches!(m.identity(), Endpoint::Added(op) if self.session.objects().iter().any(|o| o.id == op)))
        {
            members
        } else {
            Vec::new()
        }
    }

    /// The prefabs the object menu lists: those whose every tag the map
    /// holds ([`Session::prefab_fits`] names a tag it lacks when one is
    /// placed).
    fn prefab_items(&self) -> Vec<sheet::PrefabItem> {
        self.prefabs
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.missing(|block, tag| {
                    self.session
                        .palette()
                        .iter()
                        .any(|i| i.block == block && i.tag == *tag)
                })
                .is_none()
            })
            .map(|(index, p)| sheet::PrefabItem {
                index,
                name: p.name.clone(),
                line: sheet::prefab_line(p.pieces.len(), p.grouped),
            })
            .collect()
    }

    /// The prefabs for `forge.status`, each with whether this map takes it
    /// and why not.
    fn prefabs_json(&self) -> Vec<Value> {
        self.prefabs
            .iter()
            .map(|p| {
                let fit = self.session.prefab_fits(p);
                json!({
                    "name":p.name, "pieces":p.pieces.len(), "grouped":p.grouped,
                    "fits":fit.is_ok(), "reason":fit.err(),
                })
            })
            .collect()
    }

    /// The active labels, for `forge.status`.
    fn labels_json(&self) -> Vec<Value> {
        self.session
            .active_labels()
            .into_iter()
            .map(|(thing, l)| json!({"thing":thing, "text":l.text, "number":l.number}))
            .collect()
    }

    /// The active groups with their members, for `forge.status`.
    fn groups_json(&self) -> Vec<Value> {
        self.session
            .groups()
            .iter()
            .map(|g| {
                json!({
                    "id":g.id, "name":g.name, "locked":g.locked, "pivot":if g.pivot_first { "first" } else { "centre" },
                    "members":g.members,
                })
            })
            .collect()
    }

    /// The active links with their ends' final indices, for `forge.status`.
    fn links_json(&self) -> Vec<Value> {
        let mut seen = std::collections::BTreeSet::new();
        let mut all = Vec::new();
        let things = self
            .session
            .teleporters()
            .iter()
            .map(|t| Linkable::Teleporter(t.index))
            .chain(
                self.session
                    .added_teleporters()
                    .iter()
                    .map(|t| Linkable::Flag(t.id)),
            )
            .chain(
                self.session
                    .objects()
                    .iter()
                    .map(|o| Linkable::Object(o.id)),
            )
            .collect::<Vec<_>>();
        for thing in things {
            for active in self.session.links_of(thing) {
                if seen.insert(active.id) {
                    all.push(active);
                }
            }
        }
        all.sort_by_key(|a| a.id);
        all.iter()
            .map(|a| match a.link {
                DocLink::Teleporter { channel, .. } => json!({
                    "id":a.id, "kind":"teleporter", "sender":a.indices[0], "receiver":a.indices[1], "channel":channel,
                }),
                DocLink::Device { .. } => json!({
                    "id":a.id, "kind":"device", "machines":a.indices, "applies":"rebuild",
                }),
            })
            .collect()
    }

    fn linkable(&self, thing: Linkable) -> String {
        match thing {
            Linkable::Teleporter(index) => self
                .session
                .teleporters()
                .iter()
                .find(|t| t.index == index)
                .map_or_else(
                    || format!("Netgame flag {index}"),
                    |t| {
                        let role = if t.sender { "sender" } else { "receiver" };
                        format!("Teleporter {role} {index}, channel {}", t.channel)
                    },
                ),
            Linkable::Flag(id) => self
                .session
                .added_teleporters()
                .iter()
                .find(|t| t.id == id)
                .map_or_else(
                    || "Teleporter".to_string(),
                    |t| {
                        let role = if t.sender { "sender" } else { "receiver" };
                        format!("Added teleporter {role}, channel {}", t.channel)
                    },
                ),
            Linkable::Object(id) => self
                .session
                .objects()
                .iter()
                .find(|o| o.id == id)
                .map_or_else(|| "Machine".into(), |o| label(&o.item)),
        }
    }

    /// The link end an add op names: an added teleporter flag or a machine.
    fn added_linkable(&self, id: OpId) -> Linkable {
        if self.session.added_teleporters().iter().any(|t| t.id == id) {
            Linkable::Flag(id)
        } else {
            Linkable::Object(id)
        }
    }

    fn endpoint(&self, end: Endpoint) -> String {
        match end {
            Endpoint::Base {
                block: BlockKind::NetgameFlags,
                index,
                ..
            } => self.linkable(Linkable::Teleporter(index)),
            Endpoint::Base { index, .. } => format!("Machine {index}"),
            Endpoint::Added(id) => self.linkable(self.added_linkable(id)),
        }
    }

    /// What the HUD kit's `forge` HUD shows this tick (docs/game/
    /// forge-firefight-ui.md, "The monitor HUD"): the tool and what its hand
    /// has, the budget, the rebuilds pending and the notice, and where its
    /// navpoints and gizmo are in the world at `now` seconds. Nothing with
    /// the monitor off.
    pub fn monitor_hud(&self, now: f32) -> crate::forge_hud::Published {
        use hud_kit_core::views::forge::{Hand, Tool as KitTool, View};
        if !self.tool.monitor {
            return crate::forge_hud::Published::default();
        }
        let (tool, hand, target) = if self.tool.nav {
            (KitTool::Nav, Hand::Idle, String::new())
        } else if self.tool.links {
            let aimed = self
                .tool
                .aimed_link
                .filter(|&t| Some(t) != self.tool.link_from);
            let hand = if self.tool.link_from.is_some() {
                Hand::Linking
            } else if aimed.is_some() {
                Hand::Aimed
            } else {
                Hand::Idle
            };
            (
                KitTool::Links,
                hand,
                aimed.map(|t| self.linkable(t)).unwrap_or_default(),
            )
        } else {
            let ai = self.tool.ai;
            let hand = if self.tool.placing.is_some() {
                Hand::Placing
            } else if self.tool.held_thing.is_some() {
                Hand::Holding
            } else if self.tool.aimed_thing.is_some() {
                Hand::Aimed
            } else {
                Hand::Idle
            };
            (
                if ai { KitTool::Ai } else { KitTool::Objects },
                hand,
                self.target().map_or_else(String::new, |(name, _, _)| name),
            )
        };
        // A held thing being turned says its heading, as the readout's.
        let changing = (self.rotating && matches!(hand, Hand::Placing | Hand::Holding))
            .then_some(self.tool.ghost)
            .flatten()
            .map(|frame| format!("Heading {}", heading_degrees(frame.rotation[0])));
        crate::forge_hud::Published {
            view: View {
                monitor: true,
                tool,
                hand,
                target,
                used: self.session.objects().len() as u32,
                limit: self.session.budget() as u32,
                rebuild: self.session.pending_rebuild() as u32,
                enemies: crate::firefight::hud::test_enemies(),
                changing,
                teach: None,
                marks: Default::default(),
            },
            notice: self.notice.clone(),
            points: self.monitor_points_of(now, self.overlays()),
        }
    }

    /// The drawings over the world that are on.
    fn overlays(&self) -> crate::forge_hud::Show {
        crate::forge_hud::Show {
            markers: self.tool.overlays.on(Overlay::Markers),
            rings: self.tool.overlays.on(Overlay::Monitors),
            gizmo: self.tool.overlays.on(Overlay::Gizmo),
        }
    }

    /// Where a link's end is: a teleporter flag's position, or a placed
    /// machine's.
    fn linkable_at(&self, thing: Linkable) -> Option<[f32; 3]> {
        match thing {
            Linkable::Teleporter(index) => self
                .session
                .teleporters()
                .iter()
                .find(|t| t.index == index)
                .map(|t| t.position),
            Linkable::Flag(id) => self
                .session
                .added_teleporters()
                .iter()
                .find(|t| t.id == id)
                .map(|t| t.position),
            Linkable::Object(id) => self
                .session
                .objects()
                .iter()
                .find(|o| o.id == id)
                .map(|o| o.frame.position),
        }
    }

    /// The world points of the monitor's navpoints and gizmo, the drawings
    /// `show` turns off left out. The held, aimed or placed thing and the
    /// link's second end are the target; the link's first end and the other
    /// seats' monitors have their own navpoints; the gizmo is at the held,
    /// placed or aimed thing.
    fn monitor_points_of(
        &self,
        now: f32,
        show: crate::forge_hud::Show,
    ) -> crate::forge_hud::Points {
        let Some(camera) = self.tool.camera else {
            return Default::default();
        };
        let links = self.tool.links;
        let nav = self.tool.nav;
        let aimed_link = self
            .tool
            .aimed_link
            .filter(|&t| Some(t) != self.tool.link_from);
        let (target, from) = if links {
            (
                aimed_link.and_then(|t| self.linkable_at(t)),
                self.tool.link_from.and_then(|t| self.linkable_at(t)),
            )
        } else {
            // A thing being placed has no place until the ghost has one.
            let at = if self.tool.placing.is_some() {
                self.tool.ghost.map(|frame| frame.position)
            } else {
                self.target().map(|(_, _, frame)| frame.position)
            };
            (at, None)
        };
        let axes = (!links && !nav && show.gizmo)
            .then(|| {
                let frame = match self.ghost() {
                    Some((frame, _)) => frame,
                    None if self.ui_up => return None,
                    None => {
                        let id = self.tool.aimed_thing?;
                        self.session.thing(id)?.frame
                    }
                };
                super::gizmo::Axes::from_frame(frame, self.local_axes(), camera.position)
            })
            .flatten();
        crate::forge_hud::Points {
            eye: camera.position,
            target: target.filter(|_| show.markers),
            from: from.filter(|_| show.markers),
            monitors: if show.rings {
                self.monitor_points(now)
            } else {
                Vec::new()
            },
            axes,
        }
    }

    /// Edits since the last save or load.
    fn dirty(&self) -> bool {
        self.session.head() != self.saved_head
    }

    /// What X opens: Properties with something aimed at or held, else the
    /// object menu (the Links tool: the aimed thing's Link dialog).
    fn x_properties(&self) -> bool {
        if self.area.is_some() || self.tool.draft.is_some() {
            true
        } else if self.tool.links {
            self.tool.link_from.is_none() && self.tool.aimed_link.is_some()
        } else {
            self.tool.aimed_thing.is_some()
                || self.tool.held_thing.is_some()
                || self.tool.placing.is_some()
                || self.tool.placing_piece.is_some()
        }
    }

    /// Whether B has something to cancel.
    fn cancellable(&self) -> bool {
        self.area.is_some()
            || self.tool.draft.is_some()
            || self.tool.held_thing.is_some()
            || self.tool.placing.is_some()
            || self.tool.placing_piece.is_some()
            || self.tool.link_from.is_some()
    }

    fn stage(
        &mut self,
        after: After,
        edit: impl FnOnce(&mut Session, &mut ForgeWorld) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("A Forge edit is waiting for the game.".into());
        }
        let edit = ForgeWorld::apply(
            &self.session,
            &self.calls,
            self.guard,
            |tag| resolve(&self.session, &self.tags, tag),
            edit,
        )?;
        self.pending = Some(Pending { edit, after });
        Ok(())
    }

    pub fn propose(
        &mut self,
        tick: u32,
        op: Op,
        was: Option<Sha256>,
    ) -> Result<ProposalReply, String> {
        if let Some(reply) = self.propose_shared(op.clone(), was) {
            return reply;
        }
        let (tx, rx) = mpsc::channel();
        self.stage(After::Proposal(tx), |s, w| {
            s.propose(w, tick, op, was).map(|_| ())
        })?;
        Ok(rx)
    }

    fn poll(&mut self, tick: u32) {
        let Some(result) = self.pending.as_mut().and_then(|p| p.edit.poll()) else {
            return;
        };
        let Pending { after, .. } = self.pending.take().expect("just polled");
        match result {
            Ok((session, ())) => {
                self.session = Arc::new(session);
                match after {
                    After::Shared(items) => self.shared_applied(&items),
                    After::Tool(command) => {
                        self.tool.applied(&command);
                        // Duplicate: the copy is the log's last op, and the
                        // monitor holds it at once where the add put it.
                        if let Command::Duplicate { frame, .. } = &command {
                            if let Some(op) = self.session.last_add_id() {
                                let id = forge::Endpoint::Added(op);
                                self.tool.take_up(&self.session, id, Some(op), *frame);
                            }
                        }
                        if let Command::Goal { goal, .. } = command
                            && self.tool.show == Show::ThisGameType
                            && !goal.in_game(self.tool.game)
                        {
                            self.tool.set_show(Show::AllGameTypes);
                            self.say("Showing all game types so the new goal is visible.".into(), tick);
                        }
                    }
                    After::Area => self.area = None,
                    After::Edited | After::Reopened => {}
                    After::Load(name) => {
                        self.area = None;
                        self.notice = Some((flows::loaded(&name), tick));
                        self.filename = Some(name);
                        self.saved_head = self.session.head();
                        self.tool.clear_selection();
                    }
                    After::Proposal(reply) => {
                        let entry = self
                            .session
                            .log()
                            .last()
                            .cloned()
                            .ok_or_else(|| "The proposal produced no log entry.".into());
                        let _ = reply.send(entry);
                        self.tool.clear_selection();
                    }
                }
            }
            Err(reason) => {
                if let After::Proposal(reply) = after {
                    let _ = reply.send(Err(reason.clone()));
                }
                self.refuse(reason, tick);
            }
        }
    }

    /// One game tick. `ui_up`: a screen is up over the world, ours or the
    /// game's (`forge_screens::ui_up`). `game`: the session's game type, as
    /// the variant record's engine index, which decides the things the
    /// Objects tool aims at.
    pub fn tick(
        &mut self,
        tick: u32,
        bsp: Option<usize>,
        ui_up: bool,
        game: Option<i32>,
        memory: &dyn guest_layout::GuestMemory,
    ) {
        self.now = tick;
        self.retire_nav();
        self.refresh_sectors();
        self.bsp = bsp;
        self.ui_up = ui_up;
        // The object actions menu follows its object: where the monitor
        // camera shows it now.
        if screens::screen(Kind::ObjectActions).is_up()
            || screens::requested() == Some(Kind::ObjectActions)
        {
            if let Ok(fov) = memory.f32(FOV_GLOBAL)
                && fov.is_finite()
                && fov > 0.1
                && fov < 3.0
            {
                self.menu_fov = fov;
            }
            screens::object_menu::aim(self.menu_point());
        } else {
            self.menu_at = None;
        }
        if self.tool.game != game {
            self.tool.game = game;
            self.area = None;
            self.tool.clear_selection();
        }
        if !self.listed && !self.busy() {
            self.listed = true;
            let data = self.data.clone();
            let session = self.session.clone();
            if let Err(reason) = self.file(move || {
                let (prefabs, skipped) = forge::load_prefabs(&data)?;
                Ok(FileResult::Listed(
                    list_variants(&data, &session)?,
                    prefabs,
                    skipped,
                ))
            }) {
                self.notice = Some((reason, tick));
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at)| tick.saturating_sub(*at) > 120)
        {
            self.notice = None;
        }
        if self.refusal.as_ref().is_some_and(|(_, at)| {
            tick.saturating_sub(*at) > crate::forge_readout::REFUSAL_TICKS
        }) {
            self.refusal = None;
        }
        self.poll_file(tick);
        self.poll_test(tick);
        self.poll(tick);
        self.net_tick(tick);
        if let Some((ask, name)) = self.ask_wait.take() {
            self.ask(ask, &name);
        }
        if !self.busy() {
            for choice in screens::take_choices() {
                if let Err(error) = self.choose(tick, choice) {
                    self.refuse(error, tick);
                }
            }
        }
        self.player = body::local_player(memory).map(|p| p.0);
        self.poll_body(tick);
        if !self.busy() {
            let mut input = controls::sample();
            input.dialog = self.ui_up;
            // D-pad Up is the body swap's, not the tool's
            // (docs/game/forge-design-slices.md, "The monitor body").
            let swap = if input.action == Some(Action::Toggle) && !input.dialog {
                input.action = None;
                self.area = None;
                true
            } else {
                self.enter_at_start
                    && self.player.is_some()
                    && matches!(self.swap, body::Swap::Biped)
            };
            if swap {
                self.enter_at_start = false;
                if let Err(why) = self.toggle_body(memory) {
                    self.refuse(why, tick);
                }
            }
            self.rotating = input.rotating && !self.ui_up;
            self.take_input(tick, input, bsp);
            let walls = bsp.and_then(|i| self.walls.get(i));
            self.hold_height = match (self.tool.held_thing, self.tool.ghost, walls) {
                (Some(_), Some(ghost), Some(walls)) => {
                    let [x, y, z] = ghost.position;
                    walls
                        .hit(ghost.position, [x, y, z - GROUND_REACH])
                        .map(|(t, _)| t * GROUND_REACH)
                }
                _ => None,
            };
            self.fly_body();
            self.poll(tick);
        }
        self.publish(memory);
    }

    fn take_input(&mut self, tick: u32, input: Input, bsp: Option<usize>) {
        if let Err(error) = self.drive_input(tick, input, bsp) {
            self.refuse(error, tick);
        }
    }

    /// Pad, keyboard and forge.input use the same tick and BSP ray.
    fn drive_input(&mut self, tick: u32, input: Input, bsp: Option<usize>) -> Result<(), String> {
        let walls = bsp.and_then(|i| self.walls.get(i));
        let surface = |camera| surface_hit(walls, camera);
        if input.action == Some(Action::Toggle) && !input.dialog {
            self.area = None;
        }
        if let Some(draft) = &mut self.area {
            let next = draft.input(
                &mut self.tool.camera,
                input,
                sheet::MOVE_STEPS[self.movement_step],
                surface,
            )?;
            return self.apply_area_input(tick, next);
        }
        if let Some(cmd) = self.tool.update_with_surface(&self.session, tick, input, |camera| {
            surface(camera).map(|(dist, _)| dist)
        })? {
            self.command(tick, cmd)?;
        }
        Ok(())
    }

    /// The session's state for the input bridge and the screens.
    fn publish(&self, memory: &dyn guest_layout::GuestMemory) {
        controls::publish(controls::State {
            active: true,
            monitor: self.tool.monitor,
            parked: self.swap.parked(),
            body: self.swap.flying().map(|(_, unit)| unit),
            screen: self.ui_up,
            camera: self.tool.camera,
            x_properties: self.x_properties(),
            cancel: self.cancellable(),
            select_on_hold: !self.tool.links
                && !self.tool.nav
                && self.tool.placing.is_none()
                && self.tool.placing_piece.is_none()
                && self.tool.draft.is_none()
                && self.tool.held_thing.is_none(),
            area: self.area.as_ref().map(area::Draft::mode),
        });
        screens::publish(Facts {
            active: true,
            monitor: self.tool.monitor,
            dirty: self.dirty(),
            pending_rebuild: self.session.pending_rebuild(),
            rotation_snap: self.rotation_step,
            movement_snap: self.movement_step,
            local_axes: self.local_axes,
            block_grid: self.tool.block_grid,
            dev_view: self.dev_view,
            tool: Some(if self.tool.nav {
                ToolKind::Nav
            } else if self.tool.ai {
                ToolKind::Ai
            } else if self.tool.links {
                ToolKind::Links
            } else {
                ToolKind::Objects
            }),
            variants: flows::rows(&self.variants),
            filename: self.filename.clone(),
            placed: self.placed(),
            folders: sheet::folder_rows(&self.session.groups()),
            map_limits: screens::map_limits::Usage::read(memory, self.guard.scenario),
            testing: self.testing(),
            waves: Some(self.wave_facts()),
            enemy_types: super::ground::enemy_types(&self.session),
        });
    }

    /// The added objects as the placed objects browser lists them.
    fn placed(&self) -> Vec<sheet::Placed> {
        let palette = self.session.palette();
        let names = groups(palette);
        let objects: Vec<_> = self
            .session
            .objects()
            .iter()
            .filter(|o| !forge::piece::is_piece_item(&o.item))
            .filter_map(|o| {
                let (group, name) = names.get(palette.iter().position(|p| *p == o.item)?)?;
                Some((o.id, *group, name.clone(), o.frame.position))
            })
            .chain(self.session.pieces().iter().map(|p| {
                let group = if p.def.shape.is_natural() {
                    Group::Natural
                } else {
                    Group::Structure
                };
                (p.id, group, screens::blockout::name(&p.def), p.frame.position)
            }))
            .collect();
        sheet::placed_rows(&objects)
    }

    fn busy(&self) -> bool {
        self.pending.is_some() || self.file_job.is_some() || self.reloading || self.copy.is_some()
    }

    fn file(
        &mut self,
        work: impl FnOnce() -> Result<FileResult, String> + Send + 'static,
    ) -> Result<(), String> {
        let (send, recv) = mpsc::sync_channel(1);
        background::spawn("forge-file", move || {
            let _ = send.send(work());
        })
        .map_err(|e| e.to_string())?;
        self.file_job = Some(recv);
        Ok(())
    }

    fn poll_file(&mut self, tick: u32) {
        let Some(recv) = &self.file_job else { return };
        let result = match recv.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("The Forge file worker stopped.".into()),
        };
        self.file_job = None;
        match result {
            Ok(FileResult::Listed(listed, prefabs, skipped)) => {
                self.variants = listed;
                self.prefabs = prefabs;
                if let Some(first) = skipped.first() {
                    self.notice = Some((format!("Not a prefab, {first}"), tick));
                }
            }
            Ok(FileResult::Prefab(name, prefabs, skipped)) => {
                let replaced = self.prefabs.iter().any(|p| p.name == name);
                self.notice = Some((
                    match skipped.first() {
                        Some(first) => format!("Saved prefab {name}. Not a prefab, {first}"),
                        None if replaced => format!("Saved prefab {name}, replacing it."),
                        None => format!("Saved prefab {name}."),
                    },
                    tick,
                ));
                self.prefabs = prefabs;
            }
            Ok(FileResult::Saved(name, listed)) => {
                let saved = flows::saved(&name);
                let def = self.session.waves_or_default();
                let text = match screens::waves::first_problem(&def, &self.session.wave_problems()) {
                    Some(problem) => format!("{saved} {problem}"),
                    None => saved,
                };
                self.notice = Some((text, tick));
                self.filename = Some(name);
                self.variants = listed;
                // No edit runs while a file job does: this is what was saved.
                self.saved_head = self.session.head();
            }
            Ok(FileResult::Taken(name)) => match screens::open(sheet::question(Ask::Overwrite, &name)) {
                Ok(()) => self.overwrite_asked = Some(name),
                Err(reason) => self.notice = Some((reason, tick)),
            },
            Ok(FileResult::Variants(notice, listed, renamed)) => {
                self.notice = Some((notice, tick));
                if let (Some((old, new)), Some(open)) = (renamed, &self.filename)
                    && flows::same_name(&old, open)
                {
                    self.filename = Some(new);
                }
                self.variants = listed;
            }
            Ok(FileResult::Loaded(pending)) => self.pending = Some(pending),
            Ok(FileResult::Ground(done)) => {
                // No edit runs while a file job does: the copy is the
                // session as it was, with the build recorded.
                let (session, built, play) = *done;
                self.session = Arc::new(session);
                self.ground_building = false;
                self.sector_lines = self
                    .session
                    .ground_report()
                    .map(|r| super::nav_world::sector_lines(&r.sectors))
                    .unwrap_or_default();
                self.sector_head = None;
                self.refresh_sectors();
                self.copy = play;
                let mut text = super::ground::notice(self.session.ground_report(), built);
                // Test waves waits for the reload onto the ground; with none
                // coming the map does not hold it.
                if self.copy.is_none() && std::mem::take(&mut self.test_after_build) {
                    if self.session.ground_loaded() {
                        if let Err(why) = self.begin_test(None) {
                            text = why;
                        }
                    } else {
                        text = format!("{text} {}", super::ground::NOT_LOADED);
                    }
                }
                self.notice = Some((text, tick));
                if let Some(answer) = self.ground_answer.take() {
                    let mut reply = self.ground_json();
                    reply["built"] = json!(built);
                    let _ = answer.send(Ok(reply));
                }
            }
            Ok(FileResult::Rebuilt(copy)) => self.copy = Some(*copy),
            Ok(FileResult::Nav(done)) => {
                // No edit runs while a file job does: the copy is the
                // session as it was, with the generation recorded.
                self.session = Arc::new(*done);
                self.nav_generating = false;
                // The bots of the loaded map walk the new mesh at once; a
                // match that loads later takes it while it is current
                // (`bot_glue`).
                self.nav_staged = Some(self.session.head());
                super::nav::offer(&self.session.document().base.map);
                let text = super::nav::notice(self.session.nav_report());
                self.notice = Some((text, tick));
                if let Some(answer) = self.nav_answer.take() {
                    let _ = answer.send(Ok(self.nav_json()));
                }
            }
            Err(reason) => {
                self.test_after_build = false;
                if std::mem::take(&mut self.ground_building)
                    && let Some(answer) = self.ground_answer.take()
                {
                    let _ = answer.send(Err(reason.clone()));
                }
                if std::mem::take(&mut self.nav_generating)
                    && let Some(answer) = self.nav_answer.take()
                {
                    let _ = answer.send(Err(reason.clone()));
                }
                self.notice = Some((reason, tick));
            }
        }
    }

    fn command(&mut self, tick: u32, cmd: Command) -> Result<(), String> {
        use Command::*;
        match cmd {
            Palette => {
                let mut menu = sheet::objects_with_ai(&self.items(), self.selected, &self.ai_items());
                if self.session.can_place_goals() {
                    menu = Some(sheet::with_goals_for(
                        menu,
                        &self.session.goals(self.tool.game),
                    ));
                }
                if self.session.has_kit() {
                    menu = Some(sheet::with_kit(menu, &self.kit_menu()));
                }
                menu = Some(screens::blockout::with_groups(menu));
                let menu = sheet::with_prefabs(menu, &self.prefab_items());
                let mut menu = menu.ok_or("This map has nothing to place.")?;
                // The AI tool opens the menu on its group.
                if self.tool.ai
                    && let Some(at) = menu.rows.iter().position(|r| r.label == sheet::AI_PAGE)
                {
                    menu.focus = at;
                }
                screens::open(menu)?;
            }
            Properties if self.tool.links => {
                let thing = self
                    .tool
                    .aimed_link
                    .ok_or("Aim at a teleporter or machine.")?;
                let active = self
                    .session
                    .links_of(thing)
                    .pop()
                    .ok_or("Not linked. Press A to start a link.")?;
                screens::open(self.link_sheet(&active))?;
                self.link_dialog = Some(active.id);
            }
            Properties if self.tool.placing_piece.is_some() => {
                return Err("Place the piece first, then open its Properties.".into());
            }
            Properties if self.tool.held_thing.is_none() && self.aimed_drop().is_some() => {
                let (zone, _) = self.aimed_drop().ok_or("Aim at a drop zone.")?;
                let zone = u8::try_from(zone).map_err(|_| "Aim at a drop zone.")?;
                let id = self.tool.aimed_thing.ok_or("Aim at a drop zone.")?;
                let (name, _, frame) = self.target().ok_or("Aim at a drop zone.")?;
                self.coordinates = Some((CoordinateTarget::Existing(id), frame));
                screens::screen(Kind::Coordinates).show(sheet::coordinates_sheet(&name, frame))?;
                screens::waves::show(screens::waves::Page::Drop(zone))?;
            }
            Properties => {
                let (name, block, frame) = self.target().ok_or("Aim at a thing of the map.")?;
                let thing = if self.tool.placing.is_some() {
                    Thing {
                        id: Endpoint::Added(OpId(0)),
                        block,
                        kind: ThingKind::Object,
                        own: false,
                        frame,
                        fields: Vec::new(),
                        changed: false,
                    }
                } else {
                    let id = self
                        .tool
                        .held_thing
                        .or(self.tool.aimed_thing)
                        .ok_or("Aim at a thing of the map.")?;
                    self.session.thing(id).ok_or("The thing is gone.")?
                };
                let extras = if self.tool.placing.is_none() {
                    self.property_extras(thing.id)
                } else {
                    sheet::Extras::default()
                };
                let mut sheet = sheet::properties_sheet_with(
                    &name,
                    &thing,
                    frame.rotation[0].to_degrees(),
                    &extras,
                )
                .ok_or_else(|| format!("{name} has no properties to set."))?;
                if self.tool.placing.is_none() {
                    sheet
                        .rows
                        .extend(sheet::group_rows(&self.group_facts(thing.id)));
                }
                if let Some(facts) = self.piece_facts(thing.id) {
                    sheet.rows.extend(screens::blockout::piece_rows(&facts));
                }
                let target = if let Some(item) = self.tool.placing {
                    CoordinateTarget::New(item)
                } else {
                    CoordinateTarget::Existing(
                        self.tool
                            .held_thing
                            .or(self.tool.aimed_thing)
                            .ok_or("Aim at a thing of the map.")?,
                    )
                };
                if self.area_default(&thing).is_some() {
                    sheet::add_area_row(&mut sheet, thing.id);
                }
                if self.tool.placing.is_none() && forge::props::has_show(&thing) {
                    sheet::add_show_row(&mut sheet, self.session.shown(thing.id));
                }
                self.coordinates = Some((target, frame));
                screens::screen(Kind::Coordinates).show(sheet::coordinates_sheet(&name, frame))?;
                if self.tool.placing.is_some() {
                    // A thing not placed yet has no actions: its
                    // Properties alone.
                    screens::open(sheet)?;
                } else {
                    // A thing of the map: its actions as a floating menu
                    // at it, Properties among them
                    // (docs/game/ui-screens.md, "The floating menu").
                    screens::screen(Kind::Properties).show(sheet.clone())?;
                    // The header is the object's name as Properties writes it.
                    let title = sheet.title.clone();
                    let menu = screens::object_menu::sheet(&title, thing.id, Some(sheet))?;
                    self.menu_at = Some(frame.position);
                    screens::object_menu::aim(self.menu_point());
                    screens::open(menu)?;
                }
            }
            Tools => screens::request(Kind::Tools)?,
            Settings => screens::request(Kind::Settings)?,
            Overlays => screens::open(sheet::overlays_with(self.tool.overlays, self.dev_view))?,
            PieceSize { .. } => return self.open_size(),
            Menu => screens::request(Kind::Menu)?,
            BuildGround => return self.start_ground(tick, false, None),
            GenerateNav => return self.start_nav(tick, None),
            TestWaves => return self.test_waves(tick, None),
            DialogUp | DialogDown | DialogAccept | DialogBack => {
                return Err("A screen is up; it takes the pad.".into());
            }
            Delete { .. } | Remove { .. } if self.delete_asked.is_none() && self.linked(&cmd) => {
                screens::open(sheet::question(Ask::DeleteLinked, ""))?;
                self.delete_asked = Some(cmd);
            }
            _ => return self.stage(After::Tool(cmd.clone()), move |s, w| run(s, w, tick, cmd)),
        }
        Ok(())
    }

    /// Whether a delete or a removal names a thing with links, which the
    /// question asks about first.
    fn linked(&self, cmd: &Command) -> bool {
        let thing = match *cmd {
            Command::Delete { id } => Linkable::Object(id),
            Command::Remove {
                id:
                    Endpoint::Base {
                        block: BlockKind::NetgameFlags,
                        index,
                        ..
                    },
            } => Linkable::Teleporter(index),
            Command::Remove {
                id: Endpoint::Added(id),
            } => self.added_linkable(id),
            _ => return false,
        };
        !self.session.links_of(thing).is_empty()
    }

    /// Carry the Coordinates frame: a new object only changes its ghost;
    /// an existing one moves in the world with no document edit yet.
    fn hold_coordinates(&mut self, tick: u32) -> Result<(), String> {
        let (target, frame) = self.coordinates.ok_or("Coordinates is not open.")?;
        match target {
            CoordinateTarget::New(_) => {
                self.tool.ghost = Some(frame);
                Ok(())
            }
            CoordinateTarget::Existing(id) => {
                let cmd = Command::HoldThing { id, frame };
                self.stage(After::Tool(cmd.clone()), move |s, w| run(s, w, tick, cmd))
            }
        }
    }

    fn coordinate_number(
        &mut self,
        tick: u32,
        axis: sheet::Coordinate,
        text: &str,
    ) -> Result<(), String> {
        let value = text
            .parse::<f32>()
            .map_err(|_| "Type a number.".to_string())?;
        let Some((_, frame)) = &mut self.coordinates else {
            return Err("Coordinates is not open.".into());
        };
        if axis.is_angle() {
            frame.rotation[axis.index()] = value.to_radians();
        } else {
            frame.position[axis.index()] = value;
        }
        self.hold_coordinates(tick)
    }

    fn nudge_coordinates(
        &mut self,
        tick: u32,
        axis: sheet::Coordinate,
        right: bool,
    ) -> Result<(), String> {
        let step = if axis.is_angle() {
            ANGLE_STEPS[self.rotation_step].max(1.0).to_radians()
        } else {
            sheet::MOVE_STEPS[self.movement_step].max(0.01)
        };
        let step = if right { step } else { -step };
        let Some((_, frame)) = &mut self.coordinates else {
            return Err("Coordinates is not open.".into());
        };
        if axis.is_angle() {
            frame.rotation[axis.index()] += step;
        } else {
            frame.position[axis.index()] += step;
        }
        self.hold_coordinates(tick)
    }

    fn finish_coordinates(&mut self, tick: u32) -> Result<(), String> {
        let (target, frame) = self.coordinates.take().ok_or("Coordinates is not open.")?;
        let cmd = match target {
            CoordinateTarget::New(item) => Command::Place { item, frame },
            CoordinateTarget::Existing(id) => Command::MoveThing { id, frame },
        };
        self.command(tick, cmd)
    }

    fn goal_frame(&self) -> Result<forge::Frame, String> {
        let mut frame = self.marker_frame()?;
        let eye = self.tool.camera.ok_or("The monitor has no view.")?;
        if let Some((distance, normal)) = surface_hit(self.bsp.and_then(|i| self.walls.get(i)), eye)
            && normal[2] >= area::FLOOR_NORMAL
        {
            frame.position = eye.point(distance);
        }
        Ok(frame)
    }

    fn marker_frame(&self) -> Result<forge::Frame, String> {
        let eye = self.tool.camera.ok_or("The monitor has no view.")?;
        Ok(forge::Frame {
            position: eye.point(MARKER_REACH),
            rotation: [eye.yaw, 0., 0.],
        })
    }

    /// The ground labels the AI and objectives page offers, each with how
    /// many are placed.
    fn ai_items(&self) -> Vec<sheet::AiItem> {
        let counts = self.session.ground_counts();
        forge::GroundLabel::ALL
            .into_iter()
            .zip(counts)
            .map(|(label, placed)| sheet::AiItem { label, placed })
            .collect()
    }

    /// The palette as the object menu lists it: each object's group, name,
    /// how many are placed and how many more fit.
    fn items(&self) -> Vec<sheet::Item> {
        let left = self
            .session
            .budget()
            .saturating_sub(self.session.objects().len());
        groups(self.session.palette())
            .into_iter()
            .enumerate()
            .map(|(i, (group, name))| sheet::Item {
                group,
                name,
                placed: self
                    .session
                    .objects()
                    .iter()
                    .filter(|o| self.session.palette().get(i) == Some(&o.item))
                    .count(),
                left,
                hidden: self
                    .session
                    .palette()
                    .get(i)
                    .is_some_and(forge::piece::is_piece_item),
            })
            .collect()
    }

    /// What the kit adds to the object menu: the items its spawners can
    /// name, and the rows each block has left.
    fn kit_menu(&self) -> sheet::KitMenu {
        let items = self
            .session
            .kit_items()
            .into_iter()
            .enumerate()
            .map(|(index, tag)| sheet::KitItem {
                index,
                group: match forge::kit::item_group(&tag) {
                    forge::kit::ItemGroup::Weapons => Group::Weapons,
                    forge::kit::ItemGroup::Vehicles => Group::Vehicles,
                    forge::kit::ItemGroup::Equipment => Group::Equipment,
                },
                name: tag
                    .path
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&tag.path)
                    .replace('_', " "),
            })
            .collect();
        let left = |block| self.session.kit_left(block).unwrap_or(0);
        sheet::KitMenu {
            items,
            items_left: left(BlockKind::NetgameEquipment),
            spawns_left: left(BlockKind::StartingLocations),
            flags_left: left(BlockKind::NetgameFlags),
        }
    }

    fn link_sheet(&self, active: &ActiveLink) -> Sheet {
        let [from, to] = active.link.endpoints();
        let writes = match active.link {
            DocLink::Teleporter { channel, .. } => format!("Channel {channel}"),
            DocLink::Device { .. } => "Its device group applies on Rebuild map.".into(),
        };
        sheet::link(&self.endpoint(from), &self.endpoint(to), &writes)
    }

    /// A screen's choice.
    fn choose(&mut self, tick: u32, choice: Choice) -> Result<(), String> {
        match choice {
            Choice::Select(i) => {
                self.selected = i;
                self.tool.select(i);
            }
            Choice::Goal(goal) => {
                let frame = self.goal_frame()?;
                return self.command(tick, Command::Goal { goal, frame });
            }
            Choice::Spawn(team) => {
                let frame = self.goal_frame()?;
                return self.command(tick, Command::Spawn { team, frame });
            }
            Choice::Teleporter(sender) => {
                let frame = self.goal_frame()?;
                return self.command(tick, Command::Teleporter { sender, frame });
            }
            Choice::Item(i) => {
                let item = self
                    .session
                    .kit_items()
                    .get(i)
                    .cloned()
                    .ok_or("That item is gone.")?;
                let frame = self.goal_frame()?;
                return self.command(tick, Command::Item { item, frame });
            }
            Choice::EditArea(id) => return self.begin_area(id),
            Choice::Marker(forge::GroundLabel::Site) => {
                return self.choose(tick, Choice::Goal(forge::Goal::Site));
            }
            // A barrier labels the object the monitor aims at (or holds),
            // opening in the first phase: Properties' label number changes
            // the phase.
            Choice::Marker(forge::GroundLabel::Barrier) => {
                let id = self.property_target()?;
                return self.command(tick, Command::Barrier { id, phase: 1 });
            }
            Choice::Marker(label) => {
                let frame = self.marker_frame()?;
                return self.command(tick, Command::Marker { label, frame });
            }
            Choice::FlyTo(id) => {
                let at = self
                    .session
                    .objects()
                    .iter()
                    .find(|o| o.id == id)
                    .ok_or("That object is gone.")?
                    .frame
                    .position;
                let eye = self.tool.camera.ok_or("The monitor has no view.")?;
                self.tool.camera = Some(eye.toward(at, FLY_DISTANCE));
            }
            Choice::FlyToGroup(id) => {
                let group = self
                    .session
                    .groups()
                    .into_iter()
                    .find(|g| g.id == id)
                    .ok_or("That group is gone.")?;
                let at: Vec<[f32; 3]> = group
                    .members
                    .iter()
                    .filter_map(|m| self.session.thing(*m))
                    .map(|t| t.frame.position)
                    .collect();
                if at.is_empty() {
                    return Err("That group is gone.".into());
                }
                let n = at.len() as f32;
                let centre: [f32; 3] =
                    std::array::from_fn(|i| at.iter().map(|p| p[i]).sum::<f32>() / n);
                let spread = at
                    .iter()
                    .map(|p| {
                        (0..3)
                            .map(|i| (p[i] - centre[i]).powi(2))
                            .sum::<f32>()
                            .sqrt()
                    })
                    .fold(0., f32::max);
                let eye = self.tool.camera.ok_or("The monitor has no view.")?;
                self.tool.camera = Some(eye.toward(centre, FLY_DISTANCE + spread));
            }
            Choice::ToggleSelected(id) => self.tool.toggle_selected(id),
            Choice::ClearLabel(id) => {
                return self.command(tick, Command::SetLabel { id, label: None });
            }
            Choice::ResetOrientation => {
                let id = self
                    .tool
                    .held_thing
                    .or(self.tool.aimed_thing)
                    .ok_or("Aim at a thing of the map.")?;
                let (_, _, mut frame) = self.target().ok_or("The thing is gone.")?;
                // Pitch and roll to zero; the yaw rounds to the rotation
                // snap, as the tool's own turning does.
                let step = sheet::ANGLE_STEPS.get(self.rotation_step).copied().unwrap_or(0.);
                let yaw = frame.rotation[0].to_degrees();
                let yaw = if step > 0. { (yaw / step).round() * step } else { yaw };
                let yaw = yaw.to_radians();
                if self.tool.placing.is_some() || self.tool.held_thing.is_some() {
                    self.tool.set_orientation(yaw);
                } else {
                    frame.rotation = [yaw, 0., 0.];
                    let cmd = match self.tool.aimed {
                        Some(id) => Command::Move { id, frame },
                        None => Command::MoveThing { id, frame },
                    };
                    return self.command(tick, cmd);
                }
            }
            Choice::Duplicate(id) => {
                // The copy is made in front of the monitor, as a prefab is
                // placed, and held at once.
                let eye = self.tool.camera.ok_or("The monitor has no view.")?;
                let frame = forge::Frame {
                    position: eye.point(MARKER_REACH),
                    rotation: [eye.yaw, 0., 0.],
                };
                return self.command(tick, Command::Duplicate { id, frame });
            }
            Choice::PlacePrefab(i) => {
                let prefab = self.prefabs.get(i).ok_or("That prefab is gone.")?;
                self.session.prefab_fits(prefab)?;
                let eye = self.tool.camera.ok_or("The monitor has no view.")?;
                let frame = forge::Frame {
                    position: eye.point(MARKER_REACH),
                    rotation: [eye.yaw, 0., 0.],
                };
                let prefab = Box::new(prefab.clone());
                return self.command(tick, Command::PlacePrefab { prefab, frame });
            }
            Choice::GroupSelection => {
                let members = self.tool.selection().to_vec();
                let taken: Vec<String> =
                    self.session.groups().into_iter().map(|g| g.name).collect();
                let name = (taken.len() + 1..)
                    .map(|n| format!("Group {n}"))
                    .find(|name| !taken.contains(name))
                    .ok_or("No name is free.")?;
                return self.command(tick, Command::Group { name, members });
            }
            Choice::Ungroup(group) => return self.command(tick, Command::Ungroup { group }),
            Choice::LockGroup(group, locked) => {
                return self.command(tick, Command::LockGroup { group, locked });
            }
            Choice::SetGroupPivot(group, pivot_first) => {
                return self.command(tick, Command::SetGroupPivot { group, pivot_first });
            }
            Choice::BuildGround => return self.command(tick, Command::BuildGround),
            Choice::TestWaves => return self.command(tick, Command::TestWaves),
            Choice::StopWaves => return self.stop_waves(tick),
            Choice::GenerateNav => return self.start_nav(tick, None),
            Choice::Rebuild => return self.start_rebuild(tick),
            Choice::Tool(t) => {
                self.tool.set_links(t == ToolKind::Links);
                self.tool.set_nav(t == ToolKind::Nav);
                self.tool.set_ai(t == ToolKind::Ai);
            }
            Choice::Set(Prop::RotationSnap, i) => {
                let step = *ANGLE_STEPS.get(i).ok_or("No such rotation snap.")?;
                self.rotation_step = i;
                self.tool.set_rotation_step(step);
            }
            Choice::Set(Prop::MovementSnap, i) => {
                let step = *sheet::MOVE_STEPS.get(i).ok_or("No such movement snap.")?;
                self.movement_step = i;
                self.tool.set_movement_step(step);
            }
            Choice::Set(Prop::Axes, i) => {
                self.local_axes = i == 1;
                self.tool.set_axes(if self.local_axes {
                    Axes::Local
                } else {
                    Axes::World
                });
            }
            Choice::Piece(shape, seed) => {
                self.tool.select_piece(forge::piece::start(shape, seed));
            }
            Choice::Resize(id) => return self.begin_resize(id),
            Choice::NudgeSize(field, right) => return self.nudge_size(field, right),
            Choice::Reroll(id) => {
                let def = forge::piece::reroll(&self.piece_of(id)?.def)?;
                return self.write_piece(tick, id, def);
            }
            Choice::Material(id, role, material) => {
                let def = forge::piece::with_material(&self.piece_of(id)?.def, role, material);
                return self.write_piece(tick, id, def);
            }
            Choice::Set(Prop::BlockGrid, i) => {
                let grid = *sheet::BLOCK_GRIDS.get(i).ok_or("No such block-out grid.")?;
                self.tool.set_block_grid(grid);
                self.setting_writes
                    .push((forge::piece::GRID_KEY, settings::Value::Int(i64::from(grid))));
            }
            Choice::Set(Prop::DevView, i) => {
                if i > 1 {
                    return Err("No such value.".into());
                }
                self.dev_view = i == 0;
                self.setting_writes
                    .push((forge::piece::DEV_VIEW_KEY, settings::Value::Bool(i == 0)));
            }
            Choice::Set(Prop::Overlay(overlay), i) => {
                if i > 1 {
                    return Err("No such value.".into());
                }
                self.tool.overlays.set(overlay, i == 0);
            }
            Choice::Set(Prop::Heading, i) => {
                let yaw = HEADINGS.get(i).ok_or("No such heading.")?.to_radians();
                if self.tool.placing.is_some() || self.tool.held_thing.is_some() {
                    self.tool.set_heading(yaw);
                } else {
                    let id = self.tool.aimed_thing.ok_or("Aim at a thing of the map.")?;
                    let (_, _, mut frame) = self.target().ok_or("The thing is gone.")?;
                    frame.rotation[0] = yaw;
                    let cmd = match self.tool.aimed {
                        Some(id) => Command::Move { id, frame },
                        None => Command::MoveThing { id, frame },
                    };
                    return self.command(tick, cmd);
                }
            }
            Choice::Set(Prop::Team, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at a spawn or goal.")?;
                return self.command(
                    tick,
                    Command::SetField {
                        id,
                        name: "team",
                        value: forge_doc::Value::Int(i as i64),
                    },
                );
            }
            Choice::Set(Prop::SpawnerTeam, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at an item spawner.")?;
                let team = i.checked_sub(1).and_then(|t| u8::try_from(t).ok());
                return self.command(tick, Command::SetTeam { id, team });
            }
            Choice::Set(Prop::SpareClips, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at an item spawner.")?;
                let clips = i.checked_sub(1).and_then(|c| u8::try_from(c).ok());
                return self.command(tick, Command::SetSpareClips { id, clips });
            }
            Choice::Set(Prop::SpawnWeight, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at a starting point.")?;
                let weight = match i {
                    0 => Some(forge_doc::SpawnWeight::Low),
                    2 => Some(forge_doc::SpawnWeight::High),
                    _ => None,
                };
                return self.command(tick, Command::SetSpawnWeight { id, weight });
            }
            Choice::Set(Prop::PlaceAtStart, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at a spawner.")?;
                return self.command(tick, Command::SetPlaceAtStart { id, on: i == 0 });
            }
            Choice::Set(Prop::Role, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at a goal.")?;
                return self.command(
                    tick,
                    Command::SetField {
                        id,
                        name: "role",
                        value: forge_doc::Value::Int(i as i64),
                    },
                );
            }
            Choice::Set(Prop::Order, i) => {
                let id = self.tool.aimed_thing.ok_or("Aim at a goal.")?;
                return self.command(
                    tick,
                    Command::SetField {
                        id,
                        name: "order",
                        value: forge_doc::Value::Int(i as i64),
                    },
                );
            }
            Choice::Set(Prop::ShowInPlay, i) => {
                if i > 1 {
                    return Err("No such value.".into());
                }
                let id = self.property_target()?;
                return self.stage(After::Edited, move |s, w| s.set_shown(w, tick, id, i == 0));
            }
            Choice::Set(Prop::Coordinates, _) => {
                return Err("Coordinates opens its own screen.".into());
            }
            Choice::Set(Prop::RespawnTime, _) => {
                return Err("Respawn time is typed, not picked.".into());
            }
            Choice::Number(field, text) => match field {
                sheet::NumberField::Coordinate(axis) => {
                    return self.coordinate_number(tick, axis, &text);
                }
                sheet::NumberField::Area(field) => return self.area_number(field, &text),
                sheet::NumberField::Size(field) => return self.size_number(field, &text),
                sheet::NumberField::Label => {
                    let id = self.property_target()?;
                    let text = text.to_ascii_lowercase();
                    if text.is_empty() {
                        return Err("Type a label, or use Clear label.".into());
                    }
                    let number = self.session.label_of(id).and_then(|l| l.number);
                    let label = forge::Label::new(text, number).map_err(|e| e.to_string())?;
                    return self.command(
                        tick,
                        Command::SetLabel {
                            id,
                            label: Some(label),
                        },
                    );
                }
                sheet::NumberField::LabelNumber => {
                    let id = self.property_target()?;
                    let have = self.session.label_of(id).ok_or("This has no label.")?;
                    let number = match text.trim() {
                        "" => None,
                        n => Some(
                            n.parse::<i32>()
                                .map_err(|_| "Type a whole number.".to_string())?,
                        ),
                    };
                    let label = forge::Label::new(have.text, number).map_err(|e| e.to_string())?;
                    return self.command(
                        tick,
                        Command::SetLabel {
                            id,
                            label: Some(label),
                        },
                    );
                }
                sheet::NumberField::PrefabName => {
                    let id = self.property_target()?;
                    let members = self.prefab_members(id);
                    if members.is_empty() {
                        return Err(
                            "Only objects placed in this session can be saved as a prefab.".into(),
                        );
                    }
                    let (data, session) = (self.data.clone(), self.session.clone());
                    return self.file(move || {
                        let prefab = session.save_prefab(&data, &text, &members)?;
                        let (list, skipped) = forge::load_prefabs(&data)?;
                        Ok(FileResult::Prefab(prefab.name, list, skipped))
                    });
                }
                // The keyboard's completion names the variant
                // (`forge_screens`); a number that got here has none.
                sheet::NumberField::WavesName => {
                    return self.edit_waves(screens::waves::Edit::Name(text));
                }
                sheet::NumberField::RenameVariant | sheet::NumberField::DuplicateVariant => {
                    return Err("That map variant is gone.".into());
                }
                sheet::NumberField::RespawnTime | sheet::NumberField::ReturnTime => {
                    let value = text
                        .parse::<i64>()
                        .map_err(|_| "Type a whole number.".to_string())?;
                    let id = self.tool.aimed_thing.ok_or("Aim at an item entry.")?;
                    let name = match field {
                        sheet::NumberField::ReturnTime => "return time",
                        _ => "respawn time",
                    };
                    return self.command(
                        tick,
                        Command::SetField {
                            id,
                            name,
                            value: forge_doc::Value::Int(value),
                        },
                    );
                }
            },
            Choice::Nudge(axis, right) => {
                return self.nudge_coordinates(tick, axis, right);
            }
            Choice::CoordinatesDone => return self.finish_coordinates(tick),
            Choice::Unlink => {
                let link = self.link_dialog.take().ok_or("The link is gone.")?;
                return self.command(tick, Command::Unlink { link });
            }
            Choice::Answer {
                ask: Ask::DeleteLinked,
                yes,
            } => {
                let cmd = self.delete_asked.take().ok_or("The thing is gone.")?;
                if yes {
                    return self.stage(After::Tool(cmd.clone()), move |s, w| run(s, w, tick, cmd));
                }
            }
            Choice::Delete(id) => {
                let object = self
                    .session
                    .thing(id)
                    .is_some_and(|t| matches!(t.kind, ThingKind::Object | ThingKind::Piece));
                let cmd = match id {
                    Endpoint::Added(op) if object => Command::Delete { id: op },
                    _ => Command::Remove { id },
                };
                return self.command(tick, cmd);
            }
            Choice::Restore(id) => {
                screens::open(sheet::question(Ask::Restore, ""))?;
                self.restore_asked = Some(id);
            }
            Choice::Answer {
                ask: Ask::Restore,
                yes,
            } => {
                let id = self.restore_asked.take().ok_or("The thing is gone.")?;
                if yes {
                    return self.command(tick, Command::Reset { id });
                }
            }
            Choice::Answer {
                ask: Ask::Overwrite,
                yes,
            } => {
                let name = self.overwrite_asked.take().ok_or("Nothing to save.")?;
                if yes {
                    return self.save(name, true);
                }
                self.notice = Some((flows::NOT_SAVED.into(), tick));
            }
            Choice::Answer {
                ask: Ask::Load,
                yes,
            } => {
                let file = self.load_asked.take().ok_or("Nothing to load.")?;
                if yes {
                    return self.load(tick, file);
                }
                self.notice = Some((flows::NOT_LOADED.into(), tick));
            }
            // Leave Game's answer is the screens' own (`forge_screens`).
            Choice::Answer {
                ask: Ask::Leave, ..
            } => {}
            Choice::Save => match flows::save(self.filename.as_deref()) {
                // The Forge menu opens the keyboard for an unnamed map before
                // it sends Save; a Save that gets here without one is refused.
                flows::SaveStep::Keyboard => return Err(flows::UNNAMED.into()),
                flows::SaveStep::Write { name, replace } => return self.save(name, replace),
            },
            Choice::SaveAs(typed) => match flows::save_as(self.filename.as_deref(), &typed)? {
                flows::SaveStep::Write { name, replace } => return self.save(name, replace),
                flows::SaveStep::Keyboard => return Err(flows::UNNAMED.into()),
            },
            Choice::Load(file) => {
                if self.dirty() {
                    self.load_asked = Some(file);
                    self.ask(Ask::Load, "");
                } else {
                    return self.load(tick, file);
                }
            }
            Choice::DeleteVariant(file) => {
                let name = self.variant_name(&file);
                self.variant_asked = Some(file);
                self.ask(Ask::DeleteVariant, &name);
            }
            Choice::Answer {
                ask: Ask::DeleteVariant,
                yes,
            } => {
                let file = self.variant_asked.take().ok_or("Nothing to delete.")?;
                if yes {
                    return self.delete_variant(file);
                }
                self.notice = Some((flows::NOT_DELETED.into(), tick));
            }
            Choice::RenameVariant(file, typed) => {
                return self.rename_variant(file, typed.trim().to_owned());
            }
            Choice::DuplicateVariant(file, typed) => {
                return self.duplicate_variant(file, typed.trim().to_owned());
            }
            Choice::Games(set) => {
                let id = self
                    .tool
                    .held_thing
                    .or(self.tool.aimed_thing)
                    .ok_or("Aim at an item entry.")?;
                return self.stage(After::Edited, move |s, w| s.set_games(w, tick, id, set));
            }
            Choice::Waves(edit) => return self.edit_waves(edit),
            Choice::Answer {
                ask: Ask::Waves(ask),
                yes,
            } => {
                if yes {
                    return self.answer_waves(ask);
                }
            }
            Choice::EnemyType(class, on) => {
                let mut next = Session::clone(&self.session);
                super::ground::set_enemy_type(&mut next, &class, on)?;
                self.session = Arc::new(next);
            }
            Choice::Notice(text) => self.notice = Some((text, tick)),
        }
        Ok(())
    }

    /// The thing a Properties row acts on: the one held or aimed at.
    fn property_target(&self) -> Result<Endpoint, String> {
        self.tool
            .held_thing
            .or(self.tool.aimed_thing)
            .ok_or_else(|| "Aim at a thing of the map.".to_string())
    }

    /// Saves the session as the map variant `name`. A name another variant
    /// has, when `replace` is not set, ends in the Replace question.
    fn save(&mut self, name: String, replace: bool) -> Result<(), String> {
        let data = self.data.clone();
        let session = self.session.clone();
        self.file(move || match session.save_variant(&data, &name, replace) {
            Ok(_) => Ok(FileResult::Saved(name, list_variants(&data, &session)?)),
            Err(variants::Error::NameTaken(_)) => Ok(FileResult::Taken(name)),
            Err(e) => Err(e.to_string()),
        })
    }

    /// The name of the listed variant of file stem `file`.
    fn variant_name(&self, file: &str) -> String {
        self.variants
            .iter()
            .find(|v| v.file == file)
            .map_or_else(|| file.to_owned(), |v| v.name.clone())
    }

    /// Opens the question `ask`; while a Forge screen is still up it opens
    /// at a later tick.
    fn ask(&mut self, ask: Ask, name: &str) {
        if screens::open(sheet::question(ask, name)).is_err() {
            self.ask_wait = Some((ask, name.to_owned()));
        }
    }

    /// Gives the saved variant of file stem `file` the name `name`. A name
    /// another variant has is the store's refusal: nothing is replaced.
    fn rename_variant(&mut self, file: String, name: String) -> Result<(), String> {
        forge_doc::variant_name(&name).map_err(|e| e.to_string())?;
        let old = self.variant_name(&file);
        let (data, session) = (self.data.clone(), self.session.clone());
        self.file(move || {
            let map = session.document().base.map.clone();
            variants::rename(&data, &map, &file, &name).map_err(|e| e.to_string())?;
            Ok(FileResult::Variants(
                flows::renamed(&name),
                list_variants(&data, &session)?,
                Some((old, name)),
            ))
        })
    }

    /// Copies the saved variant of file stem `file` under `name`.
    fn duplicate_variant(&mut self, file: String, name: String) -> Result<(), String> {
        forge_doc::variant_name(&name).map_err(|e| e.to_string())?;
        let (data, session) = (self.data.clone(), self.session.clone());
        self.file(move || {
            let map = session.document().base.map.clone();
            variants::duplicate(&data, &map, &file, &name).map_err(|e| e.to_string())?;
            Ok(FileResult::Variants(
                flows::duplicated(&name),
                list_variants(&data, &session)?,
                None,
            ))
        })
    }

    /// Removes the saved variant of file stem `file`. The open variant is
    /// refused here as its dialog has no row for it.
    fn delete_variant(&mut self, file: String) -> Result<(), String> {
        let name = self.variant_name(&file);
        if self
            .filename
            .as_deref()
            .is_some_and(|open| flows::same_name(open, &name))
        {
            return Err(flows::OPEN_CANNOT_DELETE.into());
        }
        let (data, session) = (self.data.clone(), self.session.clone());
        self.file(move || {
            let map = session.document().base.map.clone();
            variants::delete(&data, &map, &file).map_err(|e| e.to_string())?;
            Ok(FileResult::Variants(
                flows::deleted(&name),
                list_variants(&data, &session)?,
                None,
            ))
        })
    }

    /// Loads the map variant of file stem `file` over the session.
    fn load(&mut self, tick: u32, file: String) -> Result<(), String> {
        // The loaded document is another map's: the test's actors go.
        self.stop_test(tick);
        let (data, session, tags, calls, guard) = (
            self.data.clone(),
            self.session.clone(),
            self.tags.clone(),
            self.calls.clone(),
            self.guard,
        );
        self.file(move || {
            let base = &session.document().base;
            let doc = variants::load(&data, &base.map, &file, Some(&base.sha256))
                .map_err(|e| e.to_string())?;
            let name = doc.title.clone();
            let edit = ForgeWorld::apply(
                &session,
                &calls,
                guard,
                |tag| resolve(&session, &tags, tag),
                |s, w| s.load_variant(w, tick, doc),
            )?;
            Ok(FileResult::Loaded(Pending {
                edit,
                after: After::Load(name),
            }))
        })
    }
}

/// The variants of the session's map on this machine, with the session's
/// base as the installed map.
fn list_variants(data: &Path, session: &Session) -> Result<Vec<Listed>, String> {
    let base = &session.document().base;
    variants::list(data, &base.map, Some(&base.sha256)).map_err(|e| e.to_string())
}

fn surface_hit(walls: Option<&bots::los::Walls>, camera: Camera) -> Option<(f32, [f32; 3])> {
    walls?
        .hit(camera.position, camera.point(forge::tool::MAX_HOLD))
        .map(|(t, normal)| (t * forge::tool::MAX_HOLD, normal))
}

/// A tool command's edit of the session: the same path the pad, the agent
/// and a peer take.
fn run(s: &mut Session, w: &mut ForgeWorld, tick: u32, cmd: Command) -> Result<(), String> {
    use Command::*;
    match cmd {
        Place { item, frame } => s.place(w, tick, item, frame).map(|_| ()),
        PlacePiece { def, frame } => s.place_piece(w, tick, def, frame).map(|_| ()),
        ResizePiece { id, def, origin } => s.set_piece(w, tick, id, def, Some(origin)),
        Move { id, frame } => s.move_object(w, tick, id, frame),
        Delete { id } => s.delete(w, tick, id),
        MoveThing { id, frame } => s.move_thing(w, tick, id, frame),
        HoldThing { id, frame } => s.hold(w, tick, id, frame),
        SetField { id, name, value } => s.set_field(w, tick, id, name, value),
        Duplicate { id, frame } => s.duplicate(w, tick, id, frame).map(|_| ()),
        Remove { id } => s.remove(w, tick, id),
        Reset { id } => s.reset(w, tick, id),
        SetTeam { id, team } => s.set_team(w, tick, id, team),
        SetSpareClips { id, clips } => s.set_spare_clips(w, tick, id, clips),
        SetSpawnWeight { id, weight } => s.set_spawn_weight(w, tick, id, weight),
        SetPlaceAtStart { id, on } => s.set_place_at_start(w, tick, id, on),
        Link { from, to } => s.link(w, tick, from, to).map(|_| ()),
        Unlink { link } => s.unlink(w, tick, link),
        Group { name, members } => s.group(w, tick, &name, &members, false).map(|_| ()),
        Ungroup { group } => s.ungroup(w, tick, group),
        LockGroup { group, locked } => s.set_group_locked(w, tick, group, locked),
        SetGroupPivot { group, pivot_first } => s.set_group_pivot(w, tick, group, pivot_first),
        SetLabel { id, label } => s.set_label(w, tick, id, label),
        Barrier { id, phase } => s.label_barrier(w, tick, id, phase),
        PlacePrefab { prefab, frame } => s.place_prefab(w, tick, &prefab, frame).map(|_| ()),
        Undo => s.undo(w, tick),
        Redo => s.redo(w, tick),
        Marker { label, frame } => s.place_marker_next(w, tick, label, frame).map(|_| ()),
        Goal { goal, frame } => s.place_goal(w, tick, goal, frame).map(|_| ()),
        Spawn { team, frame } => s.place_spawn(w, tick, team, frame).map(|_| ()),
        Teleporter { sender, frame } => s.place_teleporter(w, tick, sender, frame).map(|_| ()),
        Item { item, frame } => s.place_item(w, tick, &item, frame).map(|_| ()),
        _ => Err("A screen's command is not an edit.".into()),
    }
}

/// A heading in whole degrees, 0 to 359.
fn heading_degrees(radians: f32) -> i32 {
    if !radians.is_finite() {
        return 0;
    }
    (radians.to_degrees().rem_euclid(360.0).round() as i32) % 360
}

/// A thing's name as the readout and the Properties header show it: the
/// tag's base name for an object and an item spawner, else its kind.
fn thing_name(thing: &Thing) -> String {
    thing
        .fields
        .iter()
        .find_map(|(_, value)| match value {
            forge::Value::Tag(Some(tag)) => Some(
                tag.path
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&tag.path)
                    .replace('_', " "),
            ),
            _ => None,
        })
        .unwrap_or_else(|| {
            let kind = thing.kind.name();
            let mut chars = kind.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
}

fn resolve(session: &Session, tags: &[u32], tag: &forge::TagKey) -> Result<u32, String> {
    session
        .palette()
        .iter()
        .position(|p| p.tag == *tag)
        .and_then(|i| tags.get(i).copied())
        .ok_or_else(|| format!("The map has no {} tag named {}.", tag.group, tag.path))
}

/// Each palette item's object-menu group and name. Items of one block
/// that share a name are told apart by the nearest folder of their path
/// that differs from the name, as `name (folder)`; the full path only when
/// that too is shared.
fn groups(items: &[PaletteItem]) -> Vec<(Group, String)> {
    let mut counts = std::collections::BTreeMap::new();
    for item in items {
        *counts.entry((item.block, label(item))).or_insert(0usize) += 1;
    }
    let names: Vec<String> = items
        .iter()
        .map(|item| {
            let name = label(item);
            if counts[&(item.block, name.clone())] > 1 {
                qualified(&item.tag.path)
            } else {
                name
            }
        })
        .collect();
    let mut qualified_counts = std::collections::BTreeMap::new();
    for (item, name) in items.iter().zip(&names) {
        *qualified_counts.entry((item.block, name)).or_insert(0usize) += 1;
    }
    items
        .iter()
        .zip(&names)
        .map(|(item, name)| {
            let group = Group::of_block(item.block);
            let name = if qualified_counts[&(item.block, name)] > 1 {
                item.tag.path.clone()
            } else {
                name.clone()
            };
            (group, name)
        })
        .collect()
}

/// `leaf (folder)`: the nearest folder of `path` that differs from its
/// leaf, both as words.
fn qualified(path: &str) -> String {
    let mut parts = path.rsplit('\\');
    let leaf = parts.next().unwrap_or(path);
    let words = |s: &str| s.replace('_', " ");
    match parts.find(|p| !p.is_empty() && *p != leaf) {
        Some(folder) => format!("{} ({})", words(leaf), words(folder)),
        None => words(leaf),
    }
}

fn linkable_json(thing: Linkable) -> Value {
    match thing {
        Linkable::Teleporter(flag) => json!({"teleporter":flag}),
        Linkable::Flag(id) => json!({"flag":id}),
        Linkable::Object(id) => json!({"object":id}),
    }
}

fn label(item: &PaletteItem) -> String {
    item.tag
        .path
        .rsplit('\\')
        .next()
        .unwrap_or(&item.tag.path)
        .replace('_', " ")
}

#[cfg(test)]
mod tests;

/// The readout's hold line for a thing carried at `at`, `height` over the
/// ground, picked up at `from`.
fn forge_readout_hold(
    at: [f32; 3],
    height: f32,
    snap: Option<f32>,
    local: bool,
    from: Option<[f32; 3]>,
) -> hud_kit_core::views::forge_readout::Hold {
    crate::forge_readout::hold(at, height, snap, local, from)
}

/// The test room's editor (`tests::editor`), for the driver's tests.
#[cfg(test)]
pub(super) fn test_editor() -> Editor {
    tests::editor()
}
