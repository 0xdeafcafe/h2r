//! One machine's whole end of a synchronous Forge session
//! (docs/game/forge-design.md, F5): the host's authority or a guest's
//! uplink, the peer that orders the stream, and the proposals the machine
//! has made and not yet seen settled.
//!
//! A node owns no session and no world. Each tick the game hands it the
//! proposals its session queued ([`crate::Session::take_outgoing`]) and
//! applies the items it returns ([`crate::Session::apply_item`]). The bytes
//! go on a [`Lane`]: the session's reliable ordered channel, with the
//! machine's [`Beat`] in its keep-alive. Nothing here reads a clock or a
//! socket.
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use forge_doc::{Endpoint, Entry, ForgeDoc, Limits, LogHead, Op, Refusal, Sha256};
use serde::Serialize;

use super::{
    Admit, Beat, Carry, Down, Framer, Host, Item, NetRefusal, Peer, PeerError, Reply,
    Role, SEATS, Seat, Stamped, State, ToHost, Up, Welcome, frame,
};
use crate::look::session::{CanDraw, Pose};
use crate::look::{MonitorLook, Refusal as LookRefusal};
use crate::{Frame, Outgoing};

/// The host's own seat.
pub const HOST_SEAT: Seat = Seat(0);
/// Applied entries a node keeps to answer a proposal with its entry.
const RECENT: usize = 256;

/// The session's reliable ordered channel to one other machine, and its
/// keep-alive. The game implements it over the co-op link
/// (`coop::Peer::forge_send`, `forge_recv`, `forge_set_beat`).
pub trait Lane {
    /// Queue bytes: they arrive once, in order. `false` when the queue is
    /// full and nothing was queued.
    fn send(&mut self, bytes: &[u8]) -> bool;
    /// The bytes that have arrived since the last call, in order.
    fn recv(&mut self) -> Vec<u8>;
    /// The position this machine reports in every keep-alive.
    fn set_beat(&mut self, beat: Option<Beat>);
    /// The other machine's last reported position.
    fn beat(&self) -> Option<Beat>;
}

impl<T: Lane + ?Sized> Lane for Box<T> {
    fn send(&mut self, bytes: &[u8]) -> bool {
        (**self).send(bytes)
    }
    fn recv(&mut self) -> Vec<u8> {
        (**self).recv()
    }
    fn set_beat(&mut self, beat: Option<Beat>) {
        (**self).set_beat(beat)
    }
    fn beat(&self) -> Option<Beat> {
        (**self).beat()
    }
}

/// Why a node cannot go on. Each ends the machine's session.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeError {
    /// This machine has not joined: no welcome yet, or none to give.
    NotJoined,
    /// The host's refusal, or a refusal this machine made of a lane.
    Refused(NetRefusal),
    Doc(Refusal),
    Peer(PeerError),
    /// Bytes on the lane that are not a message.
    Wire(Refusal),
    Protocol(&'static str),
    /// The session had already ended, and why.
    Ended(String),
}

impl NodeError {
    /// The sentence the agent API gives for the error: its name
    /// (`agent_proto::forge::refusal`), a colon, then the words of
    /// [`fmt::Display`].
    pub fn named(&self) -> String {
        match self {
            Self::NotJoined => NetRefusal::NotJoined.named(),
            Self::Refused(refusal) => refusal.named(),
            Self::Doc(refusal) | Self::Wire(refusal) => NetRefusal::Doc(refusal.clone()).named(),
            other => format!("forge_refused: {other}"),
        }
    }
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotJoined => write!(f, "You are not in this Forge session."),
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::Doc(refusal) | Self::Wire(refusal) => write!(f, "{refusal}"),
            Self::Peer(e) => write!(f, "{e}"),
            Self::Protocol(reason) => write!(f, "The Forge channel broke its protocol: {reason}."),
            Self::Ended(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for NodeError {}

/// What one tick brings the game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    /// The items due at the tick, in stream order, for the game to apply
    /// to its session before the tick runs. A watermark is not among them.
    pub items: Vec<Stamped>,
    /// The host has not yet said that nothing is due at the tick: the game
    /// holds it (the synchronous protocol runs a tick on every machine
    /// with the same items).
    pub waiting: bool,
    /// The welcome arrived this tick: the state it gives, before any item
    /// of the tick. The game takes it as the session's
    /// ([`crate::Session::sync_to`]) and then applies `items`.
    pub joined: Option<Box<State>>,
}

/// A proposal or a hold settled: the stream carried it and this machine
/// applied it, or the host refused it.
#[derive(Clone, Debug, PartialEq)]
pub struct Settled {
    pub nonce: u32,
    /// The entry a proposal made (`None` for a hold), or the refusal's
    /// sentence led by its code (`NetRefusal::named`).
    pub result: Result<Option<Entry>, String>,
}

#[derive(Debug)]
struct Remote<L> {
    lane: L,
    framer: Framer,
    role: Role,
    joined: bool,
}

#[derive(Debug)]
struct Uplink<L> {
    lane: L,
    framer: Framer,
}

#[derive(Clone, Copy, Debug)]
struct Asked {
    hold: bool,
    /// The stream item that carries it, once the host has said.
    n: Option<u64>,
}

pub struct Node<A: Admit, L: Lane> {
    name: String,
    limits: Limits,
    host: Option<Host<A>>,
    remotes: BTreeMap<Seat, Remote<L>>,
    uplink: Option<Uplink<L>>,
    peer: Option<Peer>,
    /// The welcome this machine joined by, until a film takes it.
    welcome: Option<Welcome>,
    nonce: u32,
    asked: BTreeMap<u32, Asked>,
    recent: VecDeque<(u64, Entry)>,
    settled: Vec<Settled>,
    /// Lanes to end, and why.
    dead: Vec<(Seat, NetRefusal)>,
    ended: Option<String>,
    /// The look and pose messages off the session's channel, beside the
    /// edit log, waiting for the game's monitor glue
    /// ([`Node::take_monitors`]).
    monitors: Vec<Monitor>,
}

/// A look or pose message off the session's channel, beside the edit log
/// (docs/game/forge-monitor-appearance.md, "Other players"): what the
/// game's monitor glue drains each tick ([`Node::take_monitors`]) and
/// what it sends ([`Node::send_look`] and the rest). A look is never a
/// log entry and edits no map.
#[derive(Clone, Debug, PartialEq)]
pub enum Monitor {
    /// A seat's hello, as the host reads it: what that machine can draw.
    Hello {
        seat: Seat,
        name: String,
        can: CanDraw,
    },
    /// A seat proposed its look (to the host), naming the source map it
    /// read the model from.
    Look {
        seat: Seat,
        look: MonitorLook,
        base: Option<String>,
    },
    /// The host accepted a seat's look (to everyone): the table as
    /// everyone draws it.
    Accepted { seat: Seat, look: MonitorLook },
    /// The host refused the seat's proposed look; the seat keeps its
    /// previous one.
    LookRefused { seat: Seat, refusal: LookRefusal },
    /// A seat's monitor pose, sampled on its render clock at `at`.
    Pose {
        seat: Seat,
        at: f32,
        pose: Pose,
        monitor: bool,
        held: Option<forge_doc::OpId>,
    },
    /// One line for the HUD strip.
    Notice { seat: Seat, text: String },
}

impl<A: Admit, L: Lane> Node<A, L> {
    /// Open a session as its host, at the host's tick `now`. `start` is
    /// the document the map was materialised from; `admit` the game's
    /// checks of what an edit leaves ([`crate::Session::admit`]); an item
    /// is stamped `delay` ticks ahead (at least one).
    pub fn host(
        start: ForgeDoc,
        limits: Limits,
        admit: A,
        name: &str,
        now: u32,
        delay: u32,
    ) -> Result<Self, NodeError> {
        let mut host = Host::new(start, limits.clone(), admit, now, delay).map_err(NodeError::Doc)?;
        let (welcome, _) = host
            .join(HOST_SEAT, name, Role::Editor)
            .map_err(NodeError::Refused)?;
        let peer = Peer::join(welcome.clone(), limits.clone()).map_err(NodeError::Peer)?;
        Ok(Self {
            name: name.into(),
            limits,
            host: Some(host),
            remotes: BTreeMap::new(),
            uplink: None,
            peer: Some(peer),
            welcome: Some(welcome),
            monitors: Vec::new(),
            nonce: 0,
            asked: BTreeMap::new(),
            recent: VecDeque::new(),
            settled: vec![],
            dead: vec![],
            ended: None,
        })
    }

    /// Take a machine's lane as `seat`. It joins when its hello arrives.
    pub fn accept(&mut self, seat: Seat, lane: L, role: Role) -> Result<(), NodeError> {
        if self.host.is_none() {
            return Err(NodeError::Protocol("only the host accepts a machine"));
        }
        if seat.0 >= SEATS || seat == HOST_SEAT || self.remotes.contains_key(&seat) {
            return Err(NodeError::Refused(NetRefusal::Seat));
        }
        self.remotes.insert(
            seat,
            Remote {
                lane,
                framer: Framer::default(),
                role,
                joined: false,
            },
        );
        Ok(())
    }

    /// A machine's lane is gone: its seat leaves the session.
    pub fn disconnect(&mut self, seat: Seat) -> Result<(), NodeError> {
        let Some(remote) = self.remotes.remove(&seat) else {
            return Ok(());
        };
        if remote.joined
            && let Some(host) = self.host.as_mut()
            && let Ok(item) = host.leave(seat)
        {
            self.deliver(vec![item], None)?;
        }
        Ok(())
    }

    /// Change a seat's role (host only).
    pub fn set_role(&mut self, seat: Seat, role: Role) -> Result<(), NodeError> {
        let host = self
            .host
            .as_mut()
            .ok_or(NodeError::Protocol("only the host sets a role"))?;
        let item = host.set_role(seat, role).map_err(NodeError::Refused)?;
        if let Some(remote) = self.remotes.get_mut(&seat) {
            remote.role = role;
        }
        self.deliver(vec![item], None)
    }

    pub fn is_host(&self) -> bool {
        self.host.is_some()
    }

    pub fn seat(&self) -> Option<Seat> {
        self.peer.as_ref().map(Peer::seat)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The session as this machine has applied it, once it has joined.
    pub fn state(&self) -> Option<&State> {
        self.peer.as_ref().map(Peer::state)
    }

    pub fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }

    /// The host, for the game's own checks (a Rebuild waits until
    /// [`Host::may_rebuild`]).
    pub fn host_ref(&self) -> Option<&Host<A>> {
        self.host.as_ref()
    }

    /// The welcome this machine joined by: the film's first record. Taken
    /// once.
    /// The look and pose messages that came off the channel since the
    /// last call, in order. Not part of the edit log.
    pub fn take_monitors(&mut self) -> Vec<Monitor> {
        std::mem::take(&mut self.monitors)
    }

    /// A guest proposes its monitor look, naming the source map it read
    /// the model from. The host's answer comes back as [`Monitor::Accepted`]
    /// or [`Monitor::LookRefused`].
    pub fn send_look_up(&mut self, look: MonitorLook, base: Option<String>) {
        self.send_up(ToHost::Look { look, base });
    }

    /// A guest's monitor pose, sampled at `at` of its render clock.
    pub fn send_pose_up(&mut self, at: f32, pose: Pose, monitor: bool, held: Option<forge_doc::OpId>) {
        self.send_up(ToHost::Pose {
            at,
            pose,
            monitor,
            held,
        });
    }

    /// The host sends the accepted look of `seat` to everyone (`None`) or
    /// to one lane (a joiner learning the table).
    pub fn send_look(&mut self, look: MonitorLook, seat: Option<Seat>) {
        match seat {
            Some(seat) => self.send_down(seat, Down::Look { seat: seat.0, look }),
            None => self.broadcast_down(Down::Look { seat: 0, look }),
        }
    }

    /// The host refuses `seat`'s proposed look, to that seat alone.
    pub fn send_look_refused(&mut self, seat: Seat, refusal: LookRefusal) {
        self.send_down(seat, Down::LookRefused { refusal });
    }

    /// The host relays `seat`'s pose to every lane but `except` (its
    /// own), or the host's own pose to every lane.
    pub fn send_pose(
        &mut self,
        seat: Seat,
        at: f32,
        pose: Pose,
        monitor: bool,
        held: Option<forge_doc::OpId>,
        except: Option<Seat>,
    ) {
        self.broadcast_except(
            Down::Pose {
                seat: seat.0,
                at,
                pose,
                monitor,
                held,
            },
            except,
        );
    }

    /// The host sends one line for the HUD strip to a seat.
    pub fn send_notice(&mut self, seat: Seat, text: String) {
        self.send_down(seat, Down::Notice(text));
    }

    fn send_up(&mut self, message: ToHost) {
        let Some(uplink) = self.uplink.as_mut() else {
            return;
        };
        if put(&mut uplink.lane, &message).is_err() {
            self.ended = Some("the Forge channel's queue is full".into());
        }
    }

    fn send_down(&mut self, seat: Seat, message: Down) {
        let Some(remote) = self.remotes.get_mut(&seat) else {
            return;
        };
        if remote.joined && put(&mut remote.lane, &message).is_err() {
            self.dead.push((seat, lane_full()));
        }
    }

    fn broadcast_down(&mut self, message: Down) {
        self.broadcast_except(message, None);
    }

    fn broadcast_except(&mut self, message: Down, except: Option<Seat>) {
        for (seat, remote) in &mut self.remotes {
            if Some(*seat) == except || !remote.joined {
                continue;
            }
            if put(&mut remote.lane, &message).is_err() {
                self.dead.push((*seat, lane_full()));
            }
        }
    }

    pub fn take_welcome(&mut self) -> Option<Welcome> {
        self.welcome.take()
    }

    /// The log entries after sequence `since`.
    pub fn log_since(&self, since: u32) -> Vec<Entry> {
        self.state()
            .map(|state| {
                state
                    .log()
                    .iter()
                    .filter(|entry| entry.seq > since)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Send the proposals a session queued. The nonces come back in order.
    pub fn submit(&mut self, outgoing: Vec<Outgoing>) -> Result<Vec<u32>, NodeError> {
        let mut nonces = Vec::with_capacity(outgoing.len());
        for out in outgoing {
            let nonce = self.next_nonce();
            let proposal = out.proposal(&self.name);
            self.send_up(Up::Propose { nonce, proposal })?;
            nonces.push(nonce);
        }
        Ok(nonces)
    }

    /// Propose one op, made at the position this machine has applied (the
    /// agent API's `forge.propose`). Its nonce settles it ([`Self::take_settled`]).
    pub fn propose(&mut self, op: Op, was: Option<Sha256>) -> Result<u32, NodeError> {
        let base_seq = self
            .state()
            .ok_or(NodeError::NotJoined)?
            .head()
            .seq;
        let nonces = self.submit(vec![Outgoing { base_seq, op, was }])?;
        Ok(nonces[0])
    }

    /// Pick `target` up.
    pub fn hold(&mut self, target: Endpoint) -> Result<u32, NodeError> {
        let nonce = self.next_nonce();
        self.send_up(Up::Hold { nonce, target })?;
        Ok(nonce)
    }

    /// Move the held thing. The host keeps at most one a tick.
    pub fn carry(&mut self, target: Endpoint, frame: Frame) -> Result<(), NodeError> {
        let frame = Carry::new(frame).map_err(NodeError::Doc)?;
        self.send_up(Up::Carry { target, frame })
    }

    /// Let go of the held thing.
    pub fn release(&mut self) -> Result<(), NodeError> {
        self.send_up(Up::Release)
    }

    /// The proposals and holds settled since the last call.
    pub fn take_settled(&mut self) -> Vec<Settled> {
        std::mem::take(&mut self.settled)
    }

    /// Proposals and holds sent and not yet settled.
    pub fn in_flight(&self) -> usize {
        self.asked.len()
    }

    /// One tick of the game, at the game's tick `tick`.
    pub fn tick(&mut self, tick: u32) -> Result<Step, NodeError> {
        if let Some(why) = &self.ended {
            return Err(NodeError::Ended(why.clone()));
        }
        let result = self.run(tick);
        if let Err(e) = &result {
            self.ended = Some(e.to_string());
        }
        result
    }

    /// The session as the agent API's `forge.status` tells it.
    pub fn report(&self) -> Report {
        let state = self.state();
        Report {
            host: self.host.is_some(),
            joined: self.peer.is_some(),
            seat: self.seat(),
            name: self.name.clone(),
            role: state
                .zip(self.seat())
                .and_then(|(state, seat)| state.members().get(&seat))
                .map(|m| m.role),
            members: state
                .map(|s| {
                    s.members()
                        .iter()
                        .map(|(seat, m)| MemberRow {
                            seat: *seat,
                            name: m.name.clone(),
                            role: m.role,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            holds: state
                .map(|s| {
                    s.holds()
                        .iter()
                        .map(|(seat, held)| HoldRow {
                            seat: *seat,
                            target: held.target,
                            carried: held.frame.is_some(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            head: state.map(State::head),
            n: state.map_or(0, State::n),
            through: self.peer.as_ref().map_or(0, Peer::through),
            in_flight: self.asked.len(),
            ended: self.ended.clone(),
        }
    }

    fn next_nonce(&mut self) -> u32 {
        self.nonce = self.nonce.wrapping_add(1);
        self.nonce
    }

    fn send_up(&mut self, up: Up) -> Result<(), NodeError> {
        if self.peer.is_none() {
            return Err(NodeError::NotJoined);
        }
        match &up {
            Up::Propose { nonce, .. } => {
                self.asked.insert(
                    *nonce,
                    Asked {
                        hold: false,
                        n: None,
                    },
                );
            }
            Up::Hold { nonce, .. } => {
                self.asked.insert(*nonce, Asked { hold: true, n: None });
            }
            Up::Carry { .. } | Up::Release => {}
        }
        if let Some(host) = self.host.as_mut() {
            let out = host.receive(HOST_SEAT, up);
            self.deliver(out.items, None)?;
            if let Some(reply) = out.reply {
                self.on_reply(reply);
            }
            Ok(())
        } else if let Some(uplink) = self.uplink.as_mut() {
            put(&mut uplink.lane, &ToHost::Up(up))
        } else {
            Err(NodeError::NotJoined)
        }
    }

    /// Hand the host's own items to every joined lane and the own peer.
    fn deliver(&mut self, items: Vec<Stamped>, skip: Option<Seat>) -> Result<(), NodeError> {
        for item in items {
            for (seat, remote) in &mut self.remotes {
                if Some(*seat) == skip || !remote.joined {
                    continue;
                }
                if put(&mut remote.lane, &Down::Item(item.clone())).is_err() {
                    self.dead.push((*seat, lane_full()));
                }
            }
            if let Some(peer) = self.peer.as_mut() {
                peer.receive(item).map_err(NodeError::Peer)?;
            }
        }
        Ok(())
    }

    fn on_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Accepted { nonce, n } => {
                if let Some(asked) = self.asked.get_mut(&nonce) {
                    asked.n = Some(n);
                }
            }
            Reply::Refused { nonce, refusal } => {
                if self.asked.remove(&nonce).is_some() {
                    self.settled.push(Settled {
                        nonce,
                        result: Err(refusal.named()),
                    });
                }
            }
        }
    }

    /// Settle what the stream has carried and this machine has applied.
    fn settle(&mut self) {
        let applied = self.state().map_or(0, State::n);
        let done: Vec<u32> = self
            .asked
            .iter()
            .filter(|(_, asked)| asked.n.is_some_and(|n| n <= applied))
            .map(|(nonce, _)| *nonce)
            .collect();
        for nonce in done {
            let Some(asked) = self.asked.remove(&nonce) else {
                continue;
            };
            let n = asked.n.unwrap_or_default();
            let result: Result<Option<Entry>, String> = if asked.hold {
                Ok(None)
            } else {
                self.recent
                    .iter()
                    .find(|(m, _)| *m == n)
                    .map(|(_, entry)| Ok(Some(entry.clone())))
                    .unwrap_or_else(|| {
                        Err("forge_refused: the entry is no longer among the recent ones".into())
                    })
            };
            self.settled.push(Settled { nonce, result });
        }
    }

    fn run(&mut self, tick: u32) -> Result<Step, NodeError> {
        let mut step = Step::default();
        if self.host.is_some() {
            self.host_io()?;
            let through = self
                .host
                .as_mut()
                .expect("a host")
                .tick(tick)
                .map_err(NodeError::Refused)?;
            if let Some(item) = through {
                self.deliver(vec![item], None)?;
            }
            self.reap()?;
        } else {
            step.joined = self.guest_io()?;
        }
        let Some(peer) = self.peer.as_mut() else {
            return Ok(step);
        };
        match peer.due(tick) {
            Ok(items) => {
                for item in &items {
                    if let Item::Entry(entry) = &item.item {
                        self.recent.push_back((item.n, entry.clone()));
                        if self.recent.len() > RECENT {
                            self.recent.pop_front();
                        }
                    }
                }
                step.items = items;
            }
            Err(PeerError::Wait { .. }) => step.waiting = true,
            Err(e) => return Err(NodeError::Peer(e)),
        }
        self.settle();
        if let Some(peer) = &self.peer {
            let beat = Some(peer.beat());
            for remote in self.remotes.values_mut() {
                remote.lane.set_beat(beat);
            }
            if let Some(uplink) = self.uplink.as_mut() {
                uplink.lane.set_beat(beat);
            }
        }
        Ok(step)
    }

    /// The host's side of every lane: take what arrived, check each
    /// machine's beat against the log.
    fn host_io(&mut self) -> Result<(), NodeError> {
        let seats: Vec<Seat> = self.remotes.keys().copied().collect();
        for seat in seats {
            let mut messages = vec![];
            if let Some(remote) = self.remotes.get_mut(&seat) {
                let bytes = remote.lane.recv();
                remote.framer.push(&bytes);
                loop {
                    match remote.framer.pop::<ToHost>() {
                        Ok(Some(message)) => messages.push(message),
                        Ok(None) => break,
                        Err(e) => {
                            self.dead.push((seat, NetRefusal::Doc(e)));
                            break;
                        }
                    }
                }
            }
            for message in messages {
                self.on_remote(seat, message)?;
            }
        }
        let host = self.host.as_ref().expect("a host");
        for (seat, remote) in &self.remotes {
            if let Some(beat) = remote.lane.beat().filter(|_| remote.joined)
                && let Err(refusal) = host.check(*seat, beat)
            {
                self.dead.push((*seat, refusal));
            }
        }
        Ok(())
    }

    fn on_remote(&mut self, seat: Seat, message: ToHost) -> Result<(), NodeError> {
        let Some(remote) = self.remotes.get_mut(&seat) else {
            return Ok(());
        };
        let host = self.host.as_mut().expect("a host");
        match message {
            ToHost::Hello { name, can } => {
                if remote.joined {
                    self.dead.push((seat, NetRefusal::Seat));
                    return Ok(());
                }
                self.monitors.push(Monitor::Hello {
                    seat,
                    name: name.clone(),
                    can,
                });
                match host.join(seat, &name, remote.role) {
                    Ok((welcome, item)) => {
                        remote.joined = true;
                        if put(&mut remote.lane, &Down::Welcome(Box::new(welcome))).is_err() {
                            self.dead.push((seat, lane_full()));
                        }
                        // The welcome carries the item for its own machine.
                        self.deliver(vec![item], Some(seat))?;
                    }
                    Err(refusal) => self.dead.push((seat, refusal)),
                }
            }
            ToHost::Look { look, base } => {
                self.monitors.push(Monitor::Look { seat, look, base });
            }
            ToHost::Pose {
                at,
                pose,
                monitor,
                held,
            } => {
                self.monitors.push(Monitor::Pose {
                    seat,
                    at,
                    pose,
                    monitor,
                    held,
                });
            }
            ToHost::Up(up) => {
                if !remote.joined {
                    self.dead.push((seat, NetRefusal::NotJoined));
                    return Ok(());
                }
                let out = host.receive(seat, up);
                if let Some(reply) = out.reply
                    && put(&mut remote.lane, &Down::Reply(reply)).is_err()
                {
                    self.dead.push((seat, lane_full()));
                }
                self.deliver(out.items, None)?;
            }
        }
        Ok(())
    }

    /// End the lanes that failed, and let their seats go.
    fn reap(&mut self) -> Result<(), NodeError> {
        while let Some((seat, refusal)) = self.dead.pop() {
            let Some(mut remote) = self.remotes.remove(&seat) else {
                continue;
            };
            let _ = put(&mut remote.lane, &Down::Ended(refusal));
            if remote.joined
                && let Some(host) = self.host.as_mut()
                && let Ok(item) = host.leave(seat)
            {
                self.deliver(vec![item], None)?;
            }
        }
        Ok(())
    }

    /// The guest's side of its lane. The state the welcome gave, when it
    /// came.
    fn guest_io(&mut self) -> Result<Option<Box<State>>, NodeError> {
        let uplink = self.uplink.as_mut().ok_or(NodeError::NotJoined)?;
        let bytes = uplink.lane.recv();
        uplink.framer.push(&bytes);
        let mut downs = vec![];
        loop {
            match uplink.framer.pop::<Down>() {
                Ok(Some(down)) => downs.push(down),
                Ok(None) => break,
                Err(e) => return Err(NodeError::Wire(e)),
            }
        }
        let mut joined = None;
        for down in downs {
            match down {
                Down::Welcome(welcome) => {
                    if self.peer.is_some() {
                        return Err(NodeError::Protocol("a second welcome"));
                    }
                    let welcome = *welcome;
                    let peer = Peer::join(welcome.clone(), self.limits.clone())
                        .map_err(NodeError::Peer)?;
                    joined = Some(Box::new(peer.state().clone()));
                    self.peer = Some(peer);
                    self.welcome = Some(welcome);
                }
                Down::Item(item) => self
                    .peer
                    .as_mut()
                    .ok_or(NodeError::Protocol("an item before the welcome"))?
                    .receive(item)
                    .map_err(NodeError::Peer)?,
                Down::Reply(reply) => self.on_reply(reply),
                Down::Look { seat, look } => {
                    self.monitors.push(Monitor::Accepted {
                        seat: Seat(seat),
                        look,
                    });
                }
                Down::LookRefused { refusal } => {
                    self.monitors.push(Monitor::LookRefused {
                        seat: HOST_SEAT,
                        refusal,
                    });
                }
                Down::Pose {
                    seat,
                    at,
                    pose,
                    monitor,
                    held,
                } => {
                    self.monitors.push(Monitor::Pose {
                        seat: Seat(seat),
                        at,
                        pose,
                        monitor,
                        held,
                    });
                }
                Down::Notice(text) => {
                    self.monitors.push(Monitor::Notice {
                        seat: HOST_SEAT,
                        text,
                    });
                }
                Down::Ended(refusal) => return Err(NodeError::Refused(refusal)),
            }
        }
        Ok(joined)
    }
}

impl<A: Admit, L: Lane> Node<A, L> {
    /// Join a session over `lane`: the hello goes first, the welcome
    /// comes back, and the node is joined from the tick it arrives.
    pub fn guest(limits: Limits, name: &str, can: CanDraw, mut lane: L) -> Result<Self, NodeError> {
        put(
            &mut lane,
            &ToHost::Hello {
                name: name.into(),
                can,
            },
        )?;
        Ok(Self {
            name: name.into(),
            limits,
            host: None,
            remotes: BTreeMap::new(),
            uplink: Some(Uplink {
                lane,
                framer: Framer::default(),
            }),
            peer: None,
            welcome: None,
            monitors: Vec::new(),
            nonce: 0,
            asked: BTreeMap::new(),
            recent: VecDeque::new(),
            settled: vec![],
            dead: vec![],
            ended: None,
        })
    }
}

fn put<L: Lane>(lane: &mut L, message: &impl Serialize) -> Result<(), NodeError> {
    let bytes = frame(message).map_err(NodeError::Wire)?;
    if lane.send(&bytes) {
        Ok(())
    } else {
        Err(NodeError::Refused(lane_full()))
    }
}

fn lane_full() -> NetRefusal {
    NetRefusal::Doc(Refusal::Limit {
        what: "channel queue".into(),
        max: 0,
    })
}

/// A seat in `forge.status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemberRow {
    pub seat: Seat,
    pub name: String,
    pub role: Role,
}

/// A seat's hold in `forge.status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HoldRow {
    pub seat: Seat,
    pub target: Endpoint,
    /// The holder has moved it since it picked it up.
    pub carried: bool,
}

/// A node as `forge.status` tells it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub host: bool,
    pub joined: bool,
    pub seat: Option<Seat>,
    pub name: String,
    pub role: Option<Role>,
    pub members: Vec<MemberRow>,
    pub holds: Vec<HoldRow>,
    /// The log head as this machine has applied it.
    pub head: Option<LogHead>,
    /// The last stream item applied.
    pub n: u64,
    /// The host's watermark this machine has heard.
    pub through: u32,
    /// Proposals and holds sent and not yet settled.
    pub in_flight: usize,
    /// Why the session ended, if it has.
    pub ended: Option<String>,
}
