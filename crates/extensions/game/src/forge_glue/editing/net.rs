//! The editor in a shared session (docs/game/forge-design.md, F5): its
//! edits go to the host as proposals, and what the host stamps is applied
//! to the session and the world at the tick it names, on every machine.
use agent::forge::net::{self as agent_net, Replies};
use forge::look::body::{Bodies, Draw, Pose};
use forge::look::film::Source;
use forge::look::session::{CanDraw, Reset, Seats, Table};
use forge::look::MonitorLook;
use forge::net::{APPLY_DELAY, HOST_SEAT, Lane, Monitor, Node, Stamped, State, film};
use forge::{Session, SessionAdmit};
use forge_doc::{Entry, Op, Sha256};
use serde_json::Value;
use std::sync::{Arc, mpsc};

use super::{After, Editor};
use crate::forge_glue::net::{self as glue, Change, Offer};

type Wire = Box<dyn Lane + Send>;

/// The palette's RGB as a lookup for the drawn bodies.
fn rgb_of(palette: &[[u8; 3]]) -> impl Fn(u8) -> [u8; 3] + '_ {
    move |index| palette.get(usize::from(index)).copied().unwrap_or([0; 3])
}

/// The render clock the channel samples and draws by, as seconds: the
/// game tick's (30 a second). Each machine's poses carry its own clock.
fn clock_of(tick: u32) -> f32 {
    tick as f32 / 30.
}

/// Film the look a seat now wears, naming the source map its model was
/// read from in this machine's catalogue (ML8).
fn film_look(bodies: &Bodies, seat: u8, look: &MonitorLook) {
    let source = bodies
        .find(look)
        .and_then(|id| bodies.models().get(id as usize))
        .map(|m| Source {
            map: m.entry.source.clone(),
            sha256: m.entry.sha256.clone(),
        });
    glue::record_look(seat, look, source);
}

/// What the local machine tells the session it can draw, and the look it
/// starts with: what [`Editor::open_shared`] needs beside the offer.
pub struct MonitorStart {
    /// The local seat's look while the session runs.
    pub look: MonitorLook,
    /// What this machine can draw, as its hello says.
    pub can: CanDraw,
    /// The source map the look's model was read from, as this machine's
    /// catalogue names it.
    pub base: Option<String>,
    /// The catalogue the host model list is built from.
    pub catalogue: Vec<forge::look::census::Entry>,
    /// The palette's RGB, read from the loaded map.
    pub palette: Vec<[u8; 3]>,
}

/// A shared session's network half.
pub(super) struct Net {
    node: Node<SessionAdmit, Wire>,
    replies: Replies,
    /// Items the node released that the session has yet to apply.
    backlog: Vec<Stamped>,
    /// The state the welcome gave, until the session takes it as its own.
    joined: Option<Box<State>>,
    /// The node ended the session.
    dead: bool,
    /// The monitor channel beside the edit log (ML7).
    monitors: Monitors,
    /// The host model list and the palette, for the drawn bodies.
    bodies: Bodies,
    palette: Vec<[u8; 3]>,
}

/// The monitor channel's state: the host's authority, when this machine
/// is the host, and the seats as this machine draws them.
struct Monitors {
    table: Option<Table>,
    seats: Seats,
}

impl Editor {
    /// Make this session a seat of the shared session the link offered:
    /// the host's, or a guest's. `name` is this machine's author name;
    /// `monitor` starts the monitor channel beside the edit log.
    pub fn open_shared(
        &mut self,
        offer: Offer,
        tick: u32,
        name: &str,
        monitor: &MonitorStart,
    ) -> Result<(), String> {
        let hosting = matches!(offer, Offer::Host(_));
        let node = match offer {
            Offer::Host(lanes) => {
                let session = (*self.session).clone().share(HOST_SEAT, name);
                let mut node = Node::host(
                    session.document().clone(),
                    session.limits().clone(),
                    session.admit(),
                    name,
                    tick,
                    APPLY_DELAY,
                )
                .map_err(|e| e.to_string())?;
                for (seat, lane) in lanes {
                    node.accept(seat, lane, forge::net::Role::Editor)
                        .map_err(|e| e.to_string())?;
                }
                self.session = Arc::new(session);
                node
            }
            Offer::Guest(lane) => Node::guest(
                self.session.limits().clone(),
                name,
                monitor.can.clone(),
                lane,
            )
            .map_err(|e| e.to_string())?,
        };
        let mut monitors = Monitors {
            table: None,
            seats: Seats::default(),
        };
        if hosting {
            monitors.table = Some(Table::host(name, monitor.look.clone()));
            monitors.seats.look(HOST_SEAT.0, monitor.look.clone());
        }
        let bodies = Bodies::new(&monitor.catalogue);
        if hosting {
            film_look(&bodies, HOST_SEAT.0, &monitor.look);
        }
        self.net = Some(Net {
            node,
            replies: Replies::default(),
            backlog: Vec::new(),
            joined: None,
            dead: false,
            monitors,
            bodies,
            palette: monitor.palette.clone(),
        });
        // A guest tells the host the look it came with, naming the source
        // map it read the model from: the host answers for everyone.
        if !hosting {
            let look = monitor.look.clone();
            let base = monitor.base.clone();
            self.propose_monitor_look(look, base);
        }
        Ok(())
    }

    pub fn is_shared(&self) -> bool {
        self.net.is_some()
    }

    /// The agent API's status, as a shared session's when it is one.
    pub(super) fn shared_status(&self, status: Value) -> Value {
        match &self.net {
            Some(net) => agent_net::status(status, &net.node.report()),
            None => status,
        }
    }

    pub fn log_since(&self, since: u32) -> Vec<Entry> {
        match &self.net {
            Some(net) => net.node.log_since(since),
            None => self
                .session
                .log()
                .iter()
                .filter(|entry| entry.seq > since)
                .cloned()
                .collect(),
        }
    }

    /// `forge.propose` on a shared session: sent to the host, answered
    /// when the stream carries the entry or the host refuses it.
    pub(super) fn propose_shared(
        &mut self,
        op: Op,
        was: Option<Sha256>,
    ) -> Option<Result<mpsc::Receiver<Result<Entry, String>>, String>> {
        let net = self.net.as_mut()?;
        if net.dead {
            return Some(Err(
                net.node
                    .report()
                    .ended
                    .map_or_else(|| "forge_not_joined: the session ended".into(), |why| {
                        format!("forge_not_joined: {why}")
                    }),
            ));
        }
        Some(
            net.node
                .propose(op, was)
                .map_err(|e| e.named())
                .map(|nonce| net.replies.wait_for(nonce)),
        )
    }

    /// One game tick of the network half: send what the editor proposed,
    /// run the node, and apply what it released. Called before the
    /// editor's own work.
    pub(super) fn net_tick(&mut self, tick: u32) {
        self.film_tick(tick);
        // The monitor channel beside the edit log: poses and looks, first
        // (it ends nothing, so a dead session still drains its lane).
        self.monitor_tick(tick);
        let Some(net) = self.net.as_mut() else { return };
        if net.dead {
            return;
        }
        // What the co-op link changed since the last tick: a seat joined,
        // one left, or the link closed.
        for change in glue::take_changes() {
            match change {
                Change::Add(seat, lane) => {
                    if let Err(e) = net.node.accept(seat, lane, forge::net::Role::Editor) {
                        net.dead = true;
                        net.replies.end(&e.to_string());
                        self.notice = Some((format!("The Forge session ended: {e}"), tick));
                        return;
                    }
                }
                // A seat that never joined, or this machine is not the
                // host: the lane is already gone either way.
                Change::Drop(seat) => {
                    let _ = net.node.disconnect(seat);
                }
                Change::End(why) => {
                    net.dead = true;
                    net.replies.end(why);
                    self.notice = Some((format!("The Forge session ended: {why}"), tick));
                    return;
                }
            }
        }
        let outgoing = Arc::make_mut(&mut self.session).take_outgoing();
        if !outgoing.is_empty()
            && let Err(e) = net.node.submit(outgoing)
        {
            self.notice = Some((e.to_string(), tick));
        }
        match net.node.tick(tick) {
            Ok(step) => {
                if let Some(welcome) = net.node.take_welcome()
                    && let Ok(payload) = film::welcome_payload(&welcome)
                {
                    glue::record_edit(payload);
                }
                if let Some(state) = step.joined {
                    net.joined = Some(state);
                }
                net.backlog.extend(step.items);
            }
            Err(e) => {
                net.dead = true;
                net.replies.end(&e.to_string());
                self.notice = Some((format!("The Forge session ended: {e}"), tick));
                return;
            }
        }
        net.replies.settle(net.node.take_settled());
        let idle = net.joined.is_none() && net.backlog.is_empty();
        if idle || self.busy() {
            return;
        }
        let Some(net) = self.net.as_mut() else { return };
        let joined = net.joined.take();
        let items = std::mem::take(&mut net.backlog);
        let seat = net.node.seat();
        let name = net.node.name().to_owned();
        let staged = items.clone();
        let result = self.stage(After::Shared(items), move |s: &mut Session, w| {
            if let Some(state) = &joined {
                s.sync_to(w, state)?;
                if let Some(seat) = seat {
                    *s = s.clone().share(seat, &name);
                }
            }
            for item in &staged {
                s.apply_item(w, item)?;
            }
            Ok(())
        });
        if let Err(reason) = result {
            self.notice = Some((reason, tick));
        }
    }

    /// The local player proposes its monitor look over the channel
    /// ([`MonitorStart`]'s `look` was the first). On the host the table
    /// answers at once; a guest keeps its previous look until the host's
    /// answer arrives.
    pub fn propose_monitor_look(&mut self, look: MonitorLook, base: Option<String>) {
        let Some(net) = self.net.as_mut() else {
            return;
        };
        match net.monitors.table.as_mut() {
            Some(table) => {
                if let Ok(accepted) = table.propose(HOST_SEAT.0, look, base.as_deref()) {
                    let accepted = accepted.clone();
                    net.monitors.seats.look(HOST_SEAT.0, accepted.clone());
                    film_look(&net.bodies, HOST_SEAT.0, &accepted);
                    net.node.send_look(HOST_SEAT.0, accepted);
                }
            }
            None => net.node.send_look_up(look, base),
        }
    }

    /// The drawn bodies of the other seats at `now` of the render clock,
    /// in the looks the host sent: what the renderer draws. The local
    /// seat's own body is never drawn in its own view.
    pub fn monitor_draws(&self, now: f32, seconds: f32) -> Vec<Draw> {
        let Some(net) = &self.net else {
            return Vec::new();
        };
        let local = net.node.seat().unwrap_or(HOST_SEAT);
        let rgb = rgb_of(&net.palette);
        net.monitors.seats
            .looks()
            .filter(|(seat, _)| *seat != local.0)
            .filter_map(|(seat, look)| {
                let (pose, _) = net.monitors.seats.body(seat, now)?;
                Some(net.bodies.draw(seat, look, pose, seconds, |index| rgb(index)))
            })
            .collect()
    }

    /// Where the other seats' monitors are: the HUD kit's navpoints over
    /// them. The local seat's own monitor is never one.
    pub fn monitor_points(&self, now: f32) -> Vec<[f32; 3]> {
        let Some(net) = &self.net else {
            return Vec::new();
        };
        let local = net.node.seat().unwrap_or(HOST_SEAT);
        net.monitors
            .seats
            .looks()
            .filter(|(seat, _)| *seat != local.0)
            .filter_map(|(seat, _)| Some(net.monitors.seats.body(seat, now)?.0.position))
            .collect()
    }

    /// One tick of the monitor channel beside the edit log: sample the
    /// local pose (15 a second), relay and apply the looks and poses that
    /// came off the channel, and hold the host's table to what every
    /// machine can draw.
    fn monitor_tick(&mut self, tick: u32) {
        let now = clock_of(tick);
        let Some(net) = self.net.as_mut() else {
            return;
        };
        // The local seat's pose, at most 15 a second (a game tick is 30).
        if tick.is_multiple_of(2) {
            let (pose, monitor) = match crate::forge_controls::eye() {
                Some(camera) => (
                    Pose {
                        position: camera.position,
                        yaw: camera.yaw,
                        pitch: camera.pitch,
                    },
                    crate::forge_controls::monitor(),
                ),
                None => (Pose::default(), false),
            };
            let seat = net.node.seat().unwrap_or(HOST_SEAT);
            glue::record_pose(tick, seat.0, pose, monitor);
            match net.monitors.table.is_some() {
                true => net.node.send_pose(seat, now, pose, monitor, None, None),
                false => net.node.send_pose_up(now, pose, monitor, None),
            }
        }
        let messages = net.node.take_monitors();
        for message in messages {
            match message {
                Monitor::Hello { seat, name, can } => {
                    let Some(table) = &mut net.monitors.table else {
                        continue;
                    };
                    match table.joined(seat.0, &name, can.clone()) {
                        Ok(()) => {
                            // The joiner learns the table as it stands...
                            for (other, look) in table.looks().collect::<Vec<_>>() {
                                net.node.send_look_to(seat, other, look.clone());
                            }
                            // ...and a look it cannot draw resets its seat,
                            // told to the player it belongs to.
                            for Reset {
                                seat: reset,
                                look,
                                notice,
                            } in table.join_reset(seat.0, &name, &can)
                            {
                                net.monitors.seats.look(reset, look.clone());
                                film_look(&net.bodies, reset, &look);
                                net.node.send_look(reset, look);
                                net.node.send_notice(seat, notice);
                            }
                        }
                        Err(_) => {
                            let _ = net.node.disconnect(seat);
                        }
                    }
                }
                Monitor::Look { seat, look, base } => {
                    let Some(table) = &mut net.monitors.table else {
                        continue;
                    };
                    match table.propose(seat.0, look, base.as_deref()) {
                        Ok(accepted) => {
                            net.monitors.seats.look(seat.0, accepted.clone());
                            film_look(&net.bodies, seat.0, &accepted);
                            net.node.send_look(seat.0, accepted);
                        }
                        Err(refusal) => net.node.send_look_refused(seat, refusal),
                    }
                }
                Monitor::Accepted { seat, look } => {
                    film_look(&net.bodies, seat.0, &look);
                    net.monitors.seats.look(seat.0, look);
                }
                Monitor::LookRefused { refusal, .. } => {
                    self.notice = Some((refusal.sentence(), tick));
                }
                Monitor::Pose {
                    seat,
                    at,
                    pose,
                    monitor,
                    ..
                } => {
                    net.monitors.seats.pose(seat.0, at, pose, monitor);
                    glue::record_pose(tick, seat.0, pose, monitor);
                    if net.monitors.table.is_some() {
                        net.node.send_pose(seat, at, pose, monitor, None, Some(seat));
                    }
                }
                Monitor::Notice { text, .. } => {
                    self.notice = Some((text, tick));
                }
            }
        }
    }

    /// A film that holds a shared session plays (docs/game/film-format.md,
    /// "The film file"): the records the playback handed over for this tick
    /// are applied as the recorded machine applied them, in order. The
    /// welcome gives the session its state; each item changes the document
    /// and the world. The film holds the items in the order applied, so no
    /// peer orders them again, and the stamp ticks are the recording's own.
    /// Records wait while an edit is being staged.
    fn film_tick(&mut self, tick: u32) {
        self.played.extend(glue::take_played());
        if self.played.is_empty() || self.busy() {
            return;
        }
        let mut joined = None;
        let mut items = Vec::new();
        for record in std::mem::take(&mut self.played) {
            let Some((&kind, bytes)) = record.split_first() else {
                continue;
            };
            match kind {
                film::WELCOME => {
                    let state = forge::net::Welcome::from_bytes(bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|welcome| {
                            forge::net::Peer::join(welcome, self.session.limits().clone())
                                .map_err(|e| e.to_string())
                        })
                        .map(|peer| Box::new(peer.state().clone()));
                    match state {
                        Ok(state) => joined = Some(state),
                        Err(reason) => {
                            self.notice = Some((format!("The film's Forge session: {reason}"), tick));
                            return;
                        }
                    }
                }
                film::ITEM => match Stamped::from_bytes(bytes) {
                    Ok(item) => items.push(item),
                    Err(e) => {
                        self.notice = Some((format!("The film's Forge session: {e}"), tick));
                        return;
                    }
                },
                _ => {}
            }
        }
        if joined.is_none() && items.is_empty() {
            return;
        }
        let staged = items.clone();
        let result = self.stage(After::Shared(items), move |s: &mut Session, w| {
            if let Some(state) = &joined {
                s.sync_to(w, state)?;
            }
            for item in &staged {
                s.apply_item(w, item)?;
            }
            Ok(())
        });
        if let Err(reason) = result {
            self.notice = Some((reason, tick));
        }
    }

    /// The staged items applied: they are the film's records.
    pub(super) fn shared_applied(&mut self, items: &[Stamped]) {
        for item in items {
            if let Ok(Some(payload)) = film::item_payload(item) {
                glue::record_edit(payload);
            }
        }
    }
}
