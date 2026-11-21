//! Translates one local input stream into tool commands. A staged session
//! becomes visible only when the queued game call has returned its handle.
use super::{GENERATION, admitted, controls};
use agent::forge::local::{ForgeEdit, ForgeWorld, Guard, ProposalReply};
use agent::sim::calls;
use forge::tool::{Command, Tool};
use forge::{PaletteItem, Session};
use forge_doc::{Entry, Op, Sha256};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering, mpsc},
};

use crate::forge_map::Opened;
use crate::ui_kit::{
    forge::{ANGLE_STEPS, Action, Dialogs, HEADINGS, Hud, Mode},
    input::Input,
    spec::ScreenSpec,
};

enum After {
    Tool(Command),
    Load(String),
    Proposal(mpsc::Sender<Result<Entry, String>>),
}

struct Pending {
    edit: ForgeEdit<()>,
    after: After,
}

enum FileResult {
    Menu(Vec<String>),
    Saved(String),
    Loaded(Pending),
}

pub(super) struct Editor {
    pub session: Arc<Session>,
    tags: Arc<[u32]>,
    walls: Vec<bots::los::Walls>,
    guard: Guard,
    calls: calls::Shared,
    tool: Tool,
    dialog: Option<Dialogs>,
    pending: Option<Pending>,
    file_job: Option<mpsc::Receiver<Result<FileResult, String>>>,
    data: PathBuf,
    filename: Option<String>,
    selected: usize,
    rotation_step: usize,
    notice: Option<(String, u32)>,
}

impl Editor {
    pub fn new(opened: Opened, data: &Path, scenario: u32, calls: calls::Shared) -> Self {
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
            tool: Tool::default(),
            dialog: None,
            pending: None,
            file_job: None,
            data: data.into(),
            filename: None,
            selected: 0,
            rotation_step: 2,
            notice: None,
        }
    }

    pub fn monitor(&self) -> bool {
        self.tool.monitor
    }

    pub fn ghost(&self) -> Option<forge::Frame> {
        if self.monitor() && self.dialog.is_none() {
            self.tool.ghost
        } else {
            None
        }
    }

    pub fn status(&self) -> Value {
        let screen = self.page().map(|s| {
            json!({
                "name":s.name, "title":s.title, "focus":(!s.rows.is_empty()).then_some(s.focus),
                "rows":s.rows.iter().map(|r| &r.label).collect::<Vec<_>>(),
            })
        });
        json!({
            "session":"local", "role":"editor", "monitor":self.tool.monitor,
            "camera":self.tool.camera.map(|c| json!({"position":c.position,"yaw":c.yaw,"pitch":c.pitch})),
            "aimed":self.tool.aimed, "held":self.tool.held, "placing":self.tool.placing,
            "screen":screen, "head":self.session.head(), "pending_rebuild":0,
            "budget":{"used":self.session.objects().len(),"limit":self.session.budget()},
            "pending_call":self.pending.is_some(), "pending_file":self.file_job.is_some(), "file":self.filename,
            "notice":self.notice.as_ref().map(|(text,_)|text)
        })
    }

    fn target(&self) -> Option<(&PaletteItem, forge::Frame)> {
        if let Some(index) = self.tool.placing {
            return Some((
                self.session.palette().get(index)?,
                self.tool.ghost.unwrap_or_default(),
            ));
        }
        let id = self.tool.held.or(self.tool.aimed)?;
        let obj = self.session.objects().iter().find(|obj| obj.id == id)?;
        Some((&obj.item, self.tool.ghost.unwrap_or(obj.frame)))
    }

    fn hud(&self) -> Hud {
        let mode = if self.tool.placing.is_some() {
            Mode::Placing
        } else if self.tool.held.is_some() {
            Mode::Holding
        } else if self.tool.aimed.is_some() {
            Mode::Aimed
        } else {
            Mode::Idle
        };
        Hud {
            mode,
            target: self
                .target()
                .map_or_else(String::new, |(item, _)| label(item)),
            used: self.session.objects().len(),
            limit: self.session.budget(),
            notice: self.notice.as_ref().map(|(text, _)| text.clone()),
        }
    }

    pub fn page(&self) -> Option<ScreenSpec> {
        if !self.tool.monitor {
            return None;
        }
        let hud = self.hud();
        Some(match &self.dialog {
            Some(d) => d.spec(&hud),
            None => hud.spec(),
        })
    }

    pub fn click(&mut self, row: usize) -> bool {
        if self.dialog.is_none() || self.busy() {
            return false;
        }
        controls::click(row);
        true
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
                    After::Tool(command) => self.tool.applied(&command),
                    After::Load(name) => {
                        self.notice = Some((format!("Loaded {name}."), tick));
                        self.filename = Some(name);
                        self.dialog = None;
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

    pub fn tick(&mut self, tick: u32, bsp: Option<usize>) {
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at)| tick.saturating_sub(*at) > 120)
        {
            self.notice = None;
        }
        self.poll_file(tick);
        self.poll(tick);
        if !self.busy() {
            if self.tool.monitor && self.tool.camera.is_none() {
                self.tool.camera = controls::eye();
            }
            let mut input = controls::sample();
            input.dialog = self.dialog.is_some();
            let result = if let (Some(row), Some(d)) = (input.click, self.dialog.as_mut()) {
                let action = d.click(row);
                self.action(tick, action)
            } else {
                let walls = bsp.and_then(|i| self.walls.get(i));
                self.tool
                    .update_with_surface(&self.session, tick, input, |c| {
                        walls?.hit(c.position, c.point(80.)).map(|(t, _)| t * 80.)
                    })
                    .and_then(|cmd| match cmd {
                        Some(cmd) => self.command(tick, cmd),
                        None => Ok(()),
                    })
            };
            if let Err(error) = result {
                self.notice = Some((error, tick));
            }
            self.poll(tick);
        }
        controls::publish(
            true,
            self.tool.monitor,
            self.dialog.is_some(),
            self.tool.camera,
        );
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
            Ok(FileResult::Menu(names)) => self.dialog = Some(Dialogs::menu(names)),
            Ok(FileResult::Saved(name)) => {
                self.notice = Some((format!("Saved {name}."), tick));
                self.filename = Some(name);
                self.dialog = None;
            }
            Ok(FileResult::Loaded(pending)) => self.pending = Some(pending),
            Err(reason) => {
                self.dialog = None;
                self.notice = Some((reason, tick));
            }
        }
    }

    fn command(&mut self, tick: u32, cmd: Command) -> Result<(), String> {
        use Command::*;
        match cmd {
            Palette => {
                self.dialog = Some(Dialogs::grouped_palette(
                    groups(self.session.palette()),
                    self.selected,
                ))
            }
            Properties => {
                let (item, frame) = self.target().ok_or("Aim at a Forge object.")?;
                self.dialog = Some(Dialogs::properties(
                    label(item),
                    self.rotation_step,
                    frame.rotation[0],
                ));
            }
            Menu => {
                let data = self.data.clone();
                return self.file(move || forge::saved_names(&data).map(FileResult::Menu));
            }
            DialogUp | DialogDown | DialogAccept | DialogBack => {
                let input = match cmd {
                    DialogUp => Input::Up,
                    DialogDown => Input::Down,
                    DialogAccept => Input::A,
                    _ => Input::B,
                };
                if let Some(d) = self.dialog.as_mut() {
                    let action = d.press(input);
                    self.action(tick, action)?;
                }
            }
            _ => {
                return self.stage(After::Tool(cmd.clone()), move |s, w| match cmd {
                    Place { item, frame } => s.place(w, tick, item, frame).map(|_| ()),
                    Move { id, frame } => s.move_object(w, tick, id, frame),
                    Delete { id } => s.delete(w, tick, id),
                    Undo => s.undo(w, tick),
                    Redo => s.redo(w, tick),
                    _ => unreachable!("dialog command routed above"),
                });
            }
        }
        Ok(())
    }

    fn action(&mut self, tick: u32, action: Action) -> Result<(), String> {
        match action {
            Action::None => return Ok(()),
            Action::Close => (),
            Action::Select(i) => {
                self.selected = i;
                self.tool.select(i);
            }
            Action::RotationStep(i) => {
                self.rotation_step = i;
                self.tool.set_rotation_step(ANGLE_STEPS[i]);
            }
            Action::Heading(i) => {
                let yaw = HEADINGS[i].to_radians();
                if self.tool.placing.is_some() || self.tool.held.is_some() {
                    self.tool.set_heading(yaw);
                } else {
                    let id = self.tool.aimed.ok_or("Aim at a Forge object.")?;
                    let (_, mut frame) = self.target().ok_or("The object is gone.")?;
                    frame.rotation[0] = yaw;
                    self.command(tick, Command::Move { id, frame })?;
                }
            }
            Action::Save | Action::SaveAs => {
                let data = self.data.clone();
                let session = self.session.clone();
                let filename = (action == Action::Save)
                    .then(|| self.filename.clone())
                    .flatten();
                return self.file(move || {
                    let name = if let Some(name) = filename {
                        session.save(&data, &name)?;
                        name
                    } else {
                        session.save_new(&data, &session.document().base.map)?
                    };
                    Ok(FileResult::Saved(name))
                });
            }
            Action::Load(name) => {
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
        }
        self.dialog = None;
        Ok(())
    }
}

fn resolve(session: &Session, tags: &[u32], tag: &forge::TagKey) -> Result<u32, String> {
    session
        .palette()
        .iter()
        .position(|p| p.tag == *tag)
        .and_then(|i| tags.get(i).copied())
        .ok_or_else(|| format!("The map has no {} tag named {}.", tag.group, tag.path))
}

fn groups(items: &[PaletteItem]) -> Vec<(String, String)> {
    let mut counts = std::collections::BTreeMap::new();
    for item in items {
        *counts.entry((item.block, label(item))).or_insert(0usize) += 1;
    }
    items
        .iter()
        .map(|item| {
            let mut group = item.block.name().replace('_', " ");
            group[..1].make_ascii_uppercase();
            let name = label(item);
            let name = if counts[&(item.block, name.clone())] > 1 {
                item.tag.path.clone()
            } else {
                name
            };
            (group, name)
        })
        .collect()
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
