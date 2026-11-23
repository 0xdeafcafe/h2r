//! The game's side of a shared Forge session (docs/game/forge-design.md,
//! F5): the lanes the co-op link gives it, the offer that opens a session
//! over them, and the film records the session writes.
//!
//! Whoever builds a co-op link between two machines offers the Forge lanes
//! it carries ([`offer_host`], [`offer_guest`]); the session opens when the
//! game's Forge feature next ticks in a synchronous network game
//! ([`crate::forge_map::admitted_shared`]).
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

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

static OFFER: Mutex<Option<Offer>> = Mutex::new(None);
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

/// Whether a link has offered a Forge channel and the game has not ended.
pub fn offered() -> bool {
    OFFERED.load(Ordering::Acquire)
}

/// The session takes the offer as it opens.
pub(super) fn take() -> Option<Offer> {
    OFFER.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The game ended: the offer and the session's claim on the network go.
pub(super) fn clear() {
    *OFFER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    OFFERED.store(false, Ordering::Release);
}

static FILM: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// A record for the film's Forge entries (`live_proto::film::stream::FORGE`):
/// the welcome the session joined by, then each stream item applied.
pub(super) fn record(payload: Vec<u8>) {
    FILM.lock().unwrap_or_else(|e| e.into_inner()).push(payload);
}

/// The film's Forge records written since the last call, in order.
pub fn take_film() -> Vec<Vec<u8>> {
    std::mem::take(&mut *FILM.lock().unwrap_or_else(|e| e.into_inner()))
}
