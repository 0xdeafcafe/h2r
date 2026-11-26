//! The game's side of a shared Forge session (docs/game/forge-design.md,
//! F5): the lanes the co-op link gives it, the offer that opens a session
//! over them, and the film records the session writes: its edits are the
//! film's extension records ([`record_edit`], [`take_edits`]), played back
//! through [`deliver_played`] and [`take_played`].
//!
//! The co-op link itself ([`crate::forge_glue::link`]) offers the Forge
//! lanes it carries ([`offer_host`], [`offer_guest`]) and keeps them
//! current as seats join and leave ([`add_seat`], [`drop_seat`], [`end`]);
//! the session opens when the game's Forge feature next ticks in a
//! synchronous network game ([`crate::forge_map::admitted_shared`]).
use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use forge::look::MonitorLook;
use forge::look::film::{Recorder, Source};
use forge::look::session::Pose;
use forge::net::{BEAT_BYTES, Beat, Lane, Seat};

const _: () = assert!(BEAT_BYTES == coop::wire::BEAT);

/// A co-op peer's Forge lane: the ordered bytes and the keep-alive's beat.
/// The game's link shares the peer, so the lane locks it for each call.
pub struct CoopLane<L: coop::link::Link>(pub Arc<Mutex<coop::Peer<L>>>);

impl<L: coop::link::Link> CoopLane<L> {
    fn peer(&self) -> std::sync::MutexGuard<'_, coop::Peer<L>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<L: coop::link::Link> Lane for CoopLane<L> {
    fn send(&mut self, bytes: &[u8]) -> bool {
        self.peer().forge_send(bytes)
    }

    fn recv(&mut self) -> Vec<u8> {
        self.peer().forge_recv()
    }

    fn set_beat(&mut self, beat: Option<Beat>) {
        self.peer().forge_set_beat(beat.map(|b| b.to_bytes()));
    }

    fn beat(&self) -> Option<Beat> {
        self.peer().forge_beat().map(|b| Beat::from_bytes(&b))
    }
}

/// What the link offers a session.
pub enum Offer {
    /// This machine hosts: one lane to each other machine, by seat.
    Host(Vec<(Seat, Box<dyn Lane + Send>)>),
    /// This machine joins over its lane to the host.
    Guest(Box<dyn Lane + Send>),
}

/// A change to the lanes after the offer was made: a seat the link adds or
/// drops, or the link closing. The session that opened on the offer takes
/// them each tick; the offer itself the link re-makes, so a session that
/// opens later finds the lanes as they stand.
pub enum Change {
    Add(Seat, Box<dyn Lane + Send>),
    Drop(Seat),
    /// The link closed: the session ends. Why.
    End(&'static str),
}

static OFFER: Mutex<Option<Offer>> = Mutex::new(None);
static CHANGES: Mutex<Vec<Change>> = Mutex::new(Vec::new());
/// A lane is offered, or a session runs on one: Forge is admitted in a
/// network game while it is set.
static OFFERED: AtomicBool = AtomicBool::new(false);

fn offer(offer: Offer) {
    *OFFER.lock().unwrap_or_else(|e| e.into_inner()) = Some(offer);
    OFFERED.store(true, Ordering::Release);
}

/// This machine hosts a shared session over `lanes`.
pub fn offer_host(lanes: Vec<(Seat, Box<dyn Lane + Send>)>) {
    offer(Offer::Host(lanes));
}

/// This machine joins the host's session over `lane`.
pub fn offer_guest(lane: Box<dyn Lane + Send>) {
    offer(Offer::Guest(lane));
}

/// A guest seat joined the link: the session running on the offer takes
/// its lane as `seat` when it next ticks.
pub fn add_seat(seat: Seat, lane: Box<dyn Lane + Send>) {
    CHANGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Change::Add(seat, lane));
}

/// A guest seat left the link: the session running on the offer drops
/// its lane.
pub fn drop_seat(seat: Seat) {
    CHANGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Change::Drop(seat));
}

/// The link closed: the offer goes, and a session running on it ends for
/// `why` when it next takes its changes.
pub fn end(why: &'static str) {
    *OFFER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    OFFERED.store(false, Ordering::Release);
    CHANGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Change::End(why));
}

/// Whether a link has offered a Forge channel and the game has not ended:
/// a session may be running on one, or waiting to
/// ([`pending`](pending)). Clear when the game ends or the link closes.
pub fn offered() -> bool {
    OFFERED.load(Ordering::Acquire)
}

/// Whether an offer waits for the session that opens next: the link
/// re-makes it every tick while its lanes stand.
pub fn pending() -> bool {
    OFFER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// The session takes the offer as it opens.
pub(super) fn take() -> Option<Offer> {
    CHANGES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    OFFER.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The lane changes waiting for the session's next tick, in order.
pub(super) fn take_changes() -> Vec<Change> {
    std::mem::take(&mut *CHANGES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The game ended: the offer, its changes and the session's claim on the
/// network go.
pub(super) fn clear() {
    *OFFER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    CHANGES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    OFFERED.store(false, Ordering::Release);
    *PRESENCE.lock().unwrap_or_else(|e| e.into_inner()) = Recorder::default();
}

static FILM: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static EDITS: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static PLAYED: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// Edit records kept and not yet taken at most. A recording takes them
/// every tick, so only a machine that records no film reaches it; the
/// oldest go.
pub const MAX_WAITING: usize = 4096;

/// A record of the session's edits, which the film keeps as its extension
/// records (`theater::extension::FORGE`): the welcome the session joined by,
/// then each stream item applied, at the tick the machine applied it.
pub(super) fn record_edit(payload: Vec<u8>) {
    let mut edits = EDITS.lock().unwrap_or_else(|e| e.into_inner());
    if edits.len() >= MAX_WAITING {
        edits.remove(0);
    }
    edits.push(payload);
}

/// The edit records kept since the last call, in the order applied
/// (`theater::record`).
pub fn take_edits() -> Vec<Vec<u8>> {
    std::mem::take(&mut *EDITS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A film's edit record, handed over by the playback before the tick it
/// names (`theater::extension::deliver`).
pub fn deliver_played(payload: Vec<u8>) {
    PLAYED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(payload);
}

/// The records handed over since the last call, in film order.
pub fn take_played() -> Vec<Vec<u8>> {
    PLAYED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect()
}

/// A record for the film's Forge entries (`live_proto::film::stream::FORGE`):
/// the monitor looks and poses.
pub(super) fn record(payload: Vec<u8>) {
    FILM.lock().unwrap_or_else(|e| e.into_inner()).push(payload);
}

/// The film's Forge records written since the last call, in order.
pub fn take_film() -> Vec<Vec<u8>> {
    std::mem::take(&mut *FILM.lock().unwrap_or_else(|e| e.into_inner()))
}

/// What the film's look and pose records have said so far, to repeat at a
/// seek checkpoint (docs/game/forge-monitor-appearance.md, "Theater").
static PRESENCE: Mutex<Recorder> = Mutex::new(Recorder::new());

/// A seat's accepted look, as this machine applies it: a film record when
/// the table changed.
pub(super) fn record_look(seat: u8, look: &MonitorLook, source: Option<Source>) {
    let payload = PRESENCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .look(seat, look.clone(), source);
    if let Some(payload) = payload {
        record(payload);
    }
}

/// A seat's monitor pose at `tick`: a film record at most 15 a second.
pub(super) fn record_pose(tick: u32, seat: u8, pose: Pose, monitor: bool) {
    let payload = PRESENCE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pose(tick, seat, pose, monitor);
    if let Some(payload) = payload {
        record(payload);
    }
}

/// The records a seek checkpoint repeats: the table and each seat's last
/// pose. The recorder adds them beside the checkpoint's own baseline.
pub fn film_checkpoint() -> Vec<Vec<u8>> {
    PRESENCE.lock().unwrap_or_else(|e| e.into_inner()).checkpoint()
}
