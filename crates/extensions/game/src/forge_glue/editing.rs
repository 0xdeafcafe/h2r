//! Translates one local input stream into tool commands. A staged session
//! becomes visible only when the queued game call has returned its handle.
//! Its screens are the game's (`crate::forge_screens`): it builds their
//! sheets and acts on their choices.
use super::{GENERATION, admitted, controls};
use agent::forge::local::{self as local_api, ForgeEdit, ForgeWorld, Guard, ProposalReply};
use agent::sim::calls;
use forge::tool::{Axes, Command, Extent, Tool};
use forge::{BlockKind, Linkable, OpId, PaletteItem, Session, Thing};
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
    local_axes: bool,
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
            local_axes: false,
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
            "parked":controls::parked().map(|unit| format!("{unit:#010x}")),
            "pending_call":self.pending.is_some(), "pending_file":self.file_job.is_some(), "file":self.filename,
            "notice":self.notice.as_ref().map(|(text,_)|text)
        })
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
            let result = self
                .tool
                .update_with_surface(&self.session, tick, input, |c| {
                    walls?.hit(c.position, c.point(80.)).map(|(t, _)| t * 80.)
                })
                .and_then(|cmd| match cmd {
                    Some(cmd) => self.command(tick, cmd),
                    None => Ok(()),
                });
            if let Err(error) = result {
                self.notice = Some((error, tick));
            }
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
            Err(reason) => self.notice = Some((reason, tick)),
        }
    }

    fn command(&mut self, tick: u32, cmd: Command) -> Result<(), String> {
        use Command::*;
        match cmd {
            Palette => {
                let sheet = sheet::objects(&self.items(), self.selected)
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
                let sheet = sheet::properties_sheet(
                    &name,
                    Group::of_block(block),
                    frame.rotation[0].to_degrees(),
                )
                .ok_or_else(|| format!("{name} has no properties to set."))?;
                screens::open(sheet)?;
            }
            Tools => screens::request(Kind::Tools)?,
            Settings => screens::request(Kind::Settings)?,
            Menu => return Err("The Forge menu is GAME MENU: press Start.".into()),
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
        Remove { id } => s.remove(w, tick, id),
        Link { from, to } => s.link(w, tick, from, to).map(|_| ()),
        Unlink { link } => s.unlink(w, tick, link),
        Undo => s.undo(w, tick),
        Redo => s.redo(w, tick),
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
