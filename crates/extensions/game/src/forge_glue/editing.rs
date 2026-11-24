//! Translates one local input stream into tool commands. A staged session
//! becomes visible only when the queued game call has returned its handle.
//! Its screens are the game's (`crate::forge_screens`): it builds their
//! sheets and acts on their choices.
use super::{GENERATION, admitted, controls};
use agent::forge::ground as ground_api;
use agent::forge::local::{self as local_api, ForgeEdit, ForgeWorld, Guard, ProposalReply};
use agent::sim::calls;
use forge::tool::{Axes, Command, Extent, Tool};
use forge::{BlockKind, Linkable, OpId, PaletteItem, Session, Thing, ThingKind};
use forge_doc::{ActiveLink, Endpoint, Entry, Link as DocLink, LogHead, Op, Sha256};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
};

use crate::forge_map::Opened;
use crate::forge_screens::{
    self as screens, Facts,
    sheet::{self, Ask, Choice, Group, Kind, Prop, Sheet, Tool as ToolKind},
};
use crate::ui_kit::{
    forge::{ANGLE_STEPS, HEADINGS, Hud, Mode},
    spec::ScreenSpec,
};

mod net;

pub(crate) use net::MonitorStart;

enum After {
    Tool(Command),
    /// Items of a shared session's stream.
    Shared(Vec<forge::net::Stamped>),
    Load(String),
    Proposal(mpsc::Sender<Result<Entry, String>>),
}

struct Pending {
    edit: ForgeEdit<()>,
    after: After,
}

enum FileResult {
    /// The saved maps, for Load.
    Names(Vec<String>),
    /// Saved as this name; the saved maps after.
    Saved(String, Vec<String>),
    Loaded(Pending),
    /// The ground build's copy of the session, and whether it built.
    Ground(Box<(Session, bool)>),
}

/// Where a ground build's answer goes when an agent asked for it.
type GroundAnswer = mpsc::Sender<Result<Value, String>>;

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
/// The overlay's ring for a label the last build could not honour.
const PROBLEM_RING: [u8; 3] = [255, 90, 0];

/// The overlay's ring for a placed ground marker: a drop green, a cover
/// yellow, a base its team's colour, and the siege labels violet.
fn ground_ring(label: forge::GroundLabel, team: u8) -> [u8; 3] {
    use forge::GroundLabel as L;
    match label {
        L::Drop => [60, 220, 90],
        L::Cover => [235, 205, 60],
        L::Base if team == 0 => [225, 70, 70],
        L::Base => [70, 110, 235],
        L::Site | L::Goal | L::Line => [190, 120, 245],
    }
}

pub(super) struct Editor {
    pub session: Arc<Session>,
    tags: Arc<[u32]>,
    walls: Vec<bots::los::Walls>,
    guard: Guard,
    calls: calls::Shared,
    tool: Tool,
    pending: Option<Pending>,
    file_job: Option<mpsc::Receiver<Result<FileResult, String>>>,
    data: PathBuf,
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
    notice: Option<(String, u32)>,
    /// The link the Link dialog shows.
    link_dialog: Option<OpId>,
    /// The delete or removal the question is about.
    delete_asked: Option<Command>,
    /// The name the overwrite question is about.
    overwrite_asked: Option<String>,
    /// The log head as last saved or loaded: edits since are not saved.
    saved_head: LogHead,
    /// The saved maps, for Load.
    saved: Vec<String>,
    /// They were asked for, at the first tick.
    listed: bool,
    /// A screen is up, ours or the game's.
    ui_up: bool,
    /// The map's palette entries the Objects palette leaves out, with why.
    refused: Vec<String>,
    /// Set in a shared session (docs/game/forge-design.md, F5).
    net: Option<net::Net>,
    /// The tick the last [`Editor::tick`] ran at, for a notice asked for
    /// between ticks.
    now: u32,
    /// The file job is a ground build (`forge_glue::ground`).
    ground_building: bool,
    /// The agent that asked for the running build, to answer when it ends.
    ground_answer: Option<GroundAnswer>,
}

impl Editor {
    pub fn new(opened: Opened, data: &Path, scenario: u32, calls: calls::Shared) -> Self {
        let saved_head = opened.session.head();
        Self {
            session: Arc::new(opened.session),
            tags: opened.tags.into(),
            walls: opened.walls,
            guard: Guard {
                scenario,
                generation: GENERATION.load(Ordering::Acquire),
                admitted,
            },
            calls,
            tool: Tool::with_extents(opened.extents.into()),
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
            notice: None,
            link_dialog: None,
            delete_asked: None,
            overwrite_asked: None,
            saved_head,
            saved: Vec::new(),
            listed: false,
            ui_up: false,
            refused: opened.refused,
            net: None,
            now: 0,
            ground_building: false,
            ground_answer: None,
        }
    }

    /// Loads the saved map `name` into the session as it opens (the Forge
    /// lobby's choice). A refusal is the editor's notice.
    pub fn load_draft(&mut self, tick: u32, name: String) {
        if let Err(reason) = self.choose(tick, Choice::Load(name)) {
            self.notice = Some((reason, tick));
        }
    }

    pub fn monitor(&self) -> bool {
        self.tool.monitor
    }

    /// The monitor camera's position, yaw and pitch, for
    /// `forge.status.monitors`.
    pub fn pose(&self) -> Option<([f32; 3], f32, f32)> {
        self.tool.camera.map(|c| (c.position, c.yaw, c.pitch))
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
                readout::aimed(
                    &thing,
                    thing_name(&thing),
                    frame.position,
                    frame.rotation[0].to_degrees(),
                    self.tool.thing_extent(&self.session, &thing).radius,
                    game,
                )
            });
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
            things: things
                .iter()
                .filter_map(|t| readout::placed(t, game))
                .collect(),
            ..readout::Scene::default()
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
            "show":self.tool.show.name(), "game":self.tool.game,
            "variant":local_api::variant_status(self.session.document(), self.filename.as_deref(), self.dirty()),
            "screen":screen, "head":self.session.head(), "pending_rebuild":self.session.pending_rebuild(),
            "tool":if self.tool.links { "links" } else { "objects" }, "fine":controls::fine(),
            "rotation_snap":ANGLE_STEPS[self.rotation_step], "movement_snap":sheet::MOVE_STEPS[self.movement_step],
            "axes":self.tool.axes.name(),
            "ui_up":self.ui_up, "dirty":self.dirty(),
            "aimed_link":self.tool.aimed_link.map(linkable_json), "link_from":self.tool.link_from.map(linkable_json),
            "teleporters":self.session.teleporters().iter().map(|t| json!({
                "flag":t.index, "sender":t.sender, "channel":t.channel, "position":t.position,
            })).collect::<Vec<_>>(),
            "links":self.links_json(),
            "budget":{"used":self.session.objects().len(),"limit":self.session.budget()},
            "palette":self.session.palette().len(), "palette_refused":self.refused,
            "ground":ground_api::status(&self.session, self.ground_facts()),
            "parked":controls::parked().map(|unit| format!("{unit:#010x}")),
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
            loaded: super::ground::loads()
                && self.session.ground_state() == forge::GroundState::Current,
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
        let session = self.session.clone();
        self.file(move || {
            let mut copy = Session::clone(&session);
            let built = super::ground::run(&mut copy, if_stale)?;
            Ok(FileResult::Ground(Box::new((copy, built))))
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

    /// The ground's marks and problems as rings for the play window's
    /// overlay: a problem first, so its colour is the one a marker at the
    /// same place shows through. Nothing with the monitor off.
    pub fn ground_rings(&self) -> Vec<(forge::Frame, [u8; 3])> {
        if !self.tool.monitor {
            return Vec::new();
        }
        let problems = self.session.ground_report().into_iter().flat_map(|r| {
            r.problems.iter().map(|p| {
                let frame = forge::Frame {
                    position: p.position,
                    rotation: [0.; 3],
                };
                (frame, PROBLEM_RING)
            })
        });
        let placed = self.session.ground_counts().iter().sum::<usize>() > 0;
        let marks = placed
            .then(|| self.session.markers().unwrap_or_default())
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.frame, ground_ring(m.label, m.team)));
        problems.chain(marks).collect()
    }

    /// What is placed, held or aimed at: its name, block and frame (the
    /// ghost's while it is carried).
    fn target(&self) -> Option<(String, BlockKind, forge::Frame)> {
        if let Some(index) = self.tool.placing {
            let item = self.session.palette().get(index)?;
            return Some((label(item), item.block, self.tool.ghost.unwrap_or_default()));
        }
        let id = self.tool.held_thing.or(self.tool.aimed_thing)?;
        let thing = self.session.thing(id)?;
        Some((
            thing_name(&thing),
            thing.block,
            self.tool.ghost.unwrap_or(thing.frame),
        ))
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
            Linkable::Object(id) => self
                .session
                .objects()
                .iter()
                .find(|o| o.id == id)
                .map_or_else(|| "Machine".into(), |o| label(&o.item)),
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
            Endpoint::Added(id) => self.linkable(Linkable::Object(id)),
        }
    }

    /// The Links tool's target line: what is aimed at, or the link in hand.
    fn link_target(&self) -> String {
        let aimed = self
            .tool
            .aimed_link
            .filter(|&t| Some(t) != self.tool.link_from);
        match (self.tool.link_from, aimed) {
            (Some(from), Some(to)) => format!("{} to {}", self.linkable(from), self.linkable(to)),
            (Some(from), None) => format!("From {}", self.linkable(from)),
            (None, Some(thing)) => self.linkable(thing),
            (None, None) => String::new(),
        }
    }

    fn hud(&self) -> Hud {
        if self.tool.links {
            let mode = if self.tool.link_from.is_some() {
                Mode::Linking
            } else if self.tool.aimed_link.is_some() {
                Mode::Aimed
            } else {
                Mode::Idle
            };
            return Hud {
                mode,
                links: true,
                target: self.link_target(),
                used: self.session.objects().len(),
                limit: self.session.budget(),
                rebuild: self.session.pending_rebuild(),
                notice: self.notice.as_ref().map(|(text, _)| text.clone()),
            };
        }
        let mode = if self.tool.placing.is_some() {
            Mode::Placing
        } else if self.tool.held_thing.is_some() {
            Mode::Holding
        } else if self.tool.aimed_thing.is_some() {
            Mode::Aimed
        } else {
            Mode::Idle
        };
        Hud {
            mode,
            target: self.target().map_or_else(String::new, |(name, _, _)| name),
            used: self.session.objects().len(),
            limit: self.session.budget(),
            rebuild: self.session.pending_rebuild(),
            notice: self.notice.as_ref().map(|(text, _)| text.clone()),
            links: false,
        }
    }

    /// The compositor's HUD (slice U3 moves it to the HUD kit). The
    /// screens are the game's, so the compositor draws none.
    pub fn page(&self) -> Option<ScreenSpec> {
        self.tool.monitor.then(|| self.hud().spec())
    }

    /// Edits since the last save or load.
    fn dirty(&self) -> bool {
        self.session.head() != self.saved_head
    }

    /// What X opens: Properties with something aimed at or held, else the
    /// object menu (the Links tool: the aimed thing's Link dialog).
    fn x_properties(&self) -> bool {
        if self.tool.links {
            self.tool.link_from.is_none() && self.tool.aimed_link.is_some()
        } else {
            self.tool.aimed_thing.is_some()
                || self.tool.held_thing.is_some()
                || self.tool.placing.is_some()
        }
    }

    /// Whether B has something to cancel.
    fn cancellable(&self) -> bool {
        self.tool.held_thing.is_some()
            || self.tool.placing.is_some()
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
                    After::Tool(command) => self.tool.applied(&command),
                    After::Load(name) => {
                        self.notice = Some((format!("Loaded {name}."), tick));
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
                self.notice = Some((reason, tick));
            }
        }
    }

    /// One game tick. `ui_up`: a screen is up over the world, ours or the
    /// game's (`forge_screens::ui_up`). `game`: the session's game type, as
    /// the variant record's engine index, which decides the things the
    /// Objects tool aims at.
    pub fn tick(&mut self, tick: u32, bsp: Option<usize>, ui_up: bool, game: Option<i32>) {
        self.now = tick;
        self.ui_up = ui_up;
        if self.tool.game != game {
            self.tool.game = game;
            self.tool.clear_selection();
        }
        if !self.listed && !self.busy() {
            self.listed = true;
            let data = self.data.clone();
            if let Err(reason) = self.file(move || forge::saved_names(&data).map(FileResult::Names))
            {
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
        self.poll_file(tick);
        self.poll(tick);
        self.net_tick(tick);
        if !self.busy() {
            for choice in screens::take_choices() {
                if let Err(error) = self.choose(tick, choice) {
                    self.notice = Some((error, tick));
                }
            }
        }
        if !self.busy() {
            if self.tool.monitor && self.tool.camera.is_none() {
                self.tool.camera = controls::eye();
            }
            let mut input = controls::sample();
            input.dialog = self.ui_up;
            let walls = bsp.and_then(|i| self.walls.get(i));
            let update = self
                .tool
                .update_with_surface(&self.session, tick, input, |c| {
                    walls?.hit(c.position, c.point(80.)).map(|(t, _)| t * 80.)
                });
            let result = update.and_then(|cmd| match cmd {
                Some(cmd) => self.command(tick, cmd),
                None => Ok(()),
            });
            let walls = bsp.and_then(|i| self.walls.get(i));
            if let Err(error) = result {
                self.notice = Some((error, tick));
            }
            self.hold_height = match (self.tool.held_thing, self.tool.ghost, walls) {
                (Some(_), Some(ghost), Some(walls)) => {
                    let [x, y, z] = ghost.position;
                    walls
                        .hit(ghost.position, [x, y, z - GROUND_REACH])
                        .map(|(t, _)| t * GROUND_REACH)
                }
                _ => None,
            };
            self.poll(tick);
        }
        self.publish();
    }

    /// The session's state for the input bridge and the screens.
    fn publish(&self) {
        controls::publish(controls::State {
            active: true,
            monitor: self.tool.monitor,
            screen: self.ui_up,
            camera: self.tool.camera,
            x_properties: self.x_properties(),
            cancel: self.cancellable(),
        });
        screens::publish(Facts {
            active: true,
            dirty: self.dirty(),
            pending_rebuild: self.session.pending_rebuild(),
            rotation_snap: self.rotation_step,
            movement_snap: self.movement_step,
            local_axes: self.local_axes,
            tool: Some(if self.tool.links {
                ToolKind::Links
            } else {
                ToolKind::Objects
            }),
            saved: self.saved.clone(),
            filename: self.filename.clone(),
        });
    }

    fn busy(&self) -> bool {
        self.pending.is_some() || self.file_job.is_some()
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
            Ok(FileResult::Names(names)) => self.saved = names,
            Ok(FileResult::Saved(name, names)) => {
                self.notice = Some((format!("Saved {name}."), tick));
                self.filename = Some(name);
                self.saved = names;
                // No edit runs while a file job does: this is what was saved.
                self.saved_head = self.session.head();
            }
            Ok(FileResult::Loaded(pending)) => self.pending = Some(pending),
            Ok(FileResult::Ground(done)) => {
                // No edit runs while a file job does: the copy is the
                // session as it was, with the build recorded.
                let (session, built) = *done;
                self.session = Arc::new(session);
                self.ground_building = false;
                let text = super::ground::notice(self.session.ground_report(), built);
                self.notice = Some((text, tick));
                if let Some(answer) = self.ground_answer.take() {
                    let mut reply = self.ground_json();
                    reply["built"] = json!(built);
                    let _ = answer.send(Ok(reply));
                }
            }
            Err(reason) => {
                if std::mem::take(&mut self.ground_building)
                    && let Some(answer) = self.ground_answer.take()
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
                let sheet = sheet::objects_with_ai(&self.items(), self.selected, &self.ai_items())
                    .ok_or("This map has nothing to place.")?;
                screens::open(sheet)?;
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
                let sheet = sheet::properties_sheet(&name, &thing, frame.rotation[0].to_degrees())
                    .ok_or_else(|| format!("{name} has no properties to set."))?;
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
                self.coordinates = Some((target, frame));
                screens::screen(Kind::Coordinates).show(sheet::coordinates_sheet(&name, frame))?;
                screens::open(sheet)?;
            }
            Tools => screens::request(Kind::Tools)?,
            Settings => screens::request(Kind::Settings)?,
            Menu => return Err("The Forge menu is GAME MENU: press Start.".into()),
            BuildGround => return self.start_ground(tick, false, None),
            TestWaves => return self.start_ground(tick, true, None),
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
            })
            .collect()
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
            Choice::Marker(label) => {
                let eye = self.tool.camera.ok_or("The monitor has no view.")?;
                let frame = forge::Frame {
                    position: eye.point(MARKER_REACH),
                    rotation: [eye.yaw, 0., 0.],
                };
                return self.command(tick, Command::Marker { label, frame });
            }
            Choice::BuildGround => return self.command(tick, Command::BuildGround),
            Choice::TestWaves => return self.command(tick, Command::TestWaves),
            Choice::Tool(t) => self.tool.set_links(t == ToolKind::Links),
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
            Choice::Set(Prop::Heading, i) => {
                let yaw = HEADINGS.get(i).ok_or("No such heading.")?.to_radians();
                if self.tool.placing.is_some() || self.tool.held_thing.is_some() {
                    self.tool.set_heading(yaw);
                } else {
                    let id = self.tool.aimed_thing.ok_or("Aim at a thing of the map.")?;
                    let (_, _, mut frame) = self.target().ok_or("The thing is gone.")?;
                    frame.rotation[0] = yaw;
                    let cmd = match id {
                        Endpoint::Added(id) => Command::Move { id, frame },
                        id => Command::MoveThing { id, frame },
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
                sheet::NumberField::RespawnTime => {
                    let value = text
                        .parse::<i64>()
                        .map_err(|_| "Type a whole number.".to_string())?;
                    let id = self.tool.aimed_thing.ok_or("Aim at an item entry.")?;
                    return self.command(
                        tick,
                        Command::SetField {
                            id,
                            name: "respawn time",
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
            Choice::Answer {
                ask: Ask::Overwrite,
                yes,
            } => {
                let name = self.overwrite_asked.take().ok_or("Nothing to save.")?;
                if yes {
                    return self.save(Some(name));
                }
            }
            // Leave Game's answer is the screens' own (`forge_screens`).
            Choice::Answer {
                ask: Ask::Leave, ..
            } => {}
            Choice::Save => return self.save(self.filename.clone()),
            Choice::SaveAs(name) => {
                if Some(&name) != self.filename.as_ref() && self.saved.contains(&name) {
                    screens::open(sheet::question(Ask::Overwrite, &name))?;
                    self.overwrite_asked = Some(name);
                } else {
                    return self.save(Some(name));
                }
            }
            Choice::Load(name) => {
                let (data, session, tags, calls, guard) = (
                    self.data.clone(),
                    self.session.clone(),
                    self.tags.clone(),
                    self.calls.clone(),
                    self.guard,
                );
                return self.file(move || {
                    let edit = ForgeWorld::apply(
                        &session,
                        &calls,
                        guard,
                        |tag| resolve(&session, &tags, tag),
                        |s, w| s.load(w, tick, &data, &name),
                    )?;
                    Ok(FileResult::Loaded(Pending {
                        edit,
                        after: After::Load(name),
                    }))
                });
            }
            Choice::Notice(text) => self.notice = Some((text, tick)),
        }
        Ok(())
    }

    /// Saves as `name`, or as a new name from the map's when none.
    fn save(&mut self, name: Option<String>) -> Result<(), String> {
        let data = self.data.clone();
        let session = self.session.clone();
        self.file(move || {
            let name = if let Some(name) = name {
                session.save(&data, &name)?;
                name
            } else {
                session.save_new(&data, &session.document().base.map)?
            };
            Ok(FileResult::Saved(name, forge::saved_names(&data)?))
        })
    }
}

/// A tool command's edit of the session: the same path the pad, the agent
/// and a peer take.
fn run(s: &mut Session, w: &mut ForgeWorld, tick: u32, cmd: Command) -> Result<(), String> {
    use Command::*;
    match cmd {
        Place { item, frame } => s.place(w, tick, item, frame).map(|_| ()),
        Move { id, frame } => s.move_object(w, tick, id, frame),
        Delete { id } => s.delete(w, tick, id),
        MoveThing { id, frame } => s.move_thing(w, tick, id, frame),
        HoldThing { id, frame } => s.hold(w, tick, id, frame),
        SetField { id, name, value } => s.set_field(w, tick, id, name, value),
        Remove { id } => s.remove(w, tick, id),
        Link { from, to } => s.link(w, tick, from, to).map(|_| ()),
        Unlink { link } => s.unlink(w, tick, link),
        Undo => s.undo(w, tick),
        Redo => s.redo(w, tick),
        Marker { label, frame } => s.place_marker_next(w, tick, label, frame).map(|_| ()),
        _ => Err("A screen's command is not an edit.".into()),
    }
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
