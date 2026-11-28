//! The pickers' upstream half on the actor channel
//! (docs/game/rules-world-api.md, "Pickers"): one player's pick, three
//! bytes, sent by the machine whose local player made it and landed in
//! the host's VM by [`receive`], which hands it to the same reports the
//! local pickers make (`crate::rules::world::pick`). The offers
//! themselves go downstream as the show channel's `Pick` cue
//! (`rules::show::Cue::Pick`); only the picks come back this way.
//!
//! The Live tunnel carries it as the client-to-host `ActorPick` frame
//! (`live_proto::tunnel::Kind::ActorPick`): the sending machine's id, then
//! these three bytes. [`send`] queues a pick for the transport and
//! `actor_net_link::send_pick` frames it; the host's
//! `actor_net_link::take_picks` hands each back with the roster slots whose
//! players have that machine id ([`seats_of`]), which [`receive`] checks
//! the pick's player against. A client learns its own machine id from its
//! local players ([`read_local_machine`]).

use std::collections::VecDeque;
use std::sync::Mutex;

use ::rules::ty::MAX_PLAYERS;
use guest_layout::GuestMemory;
use guest_layout::view::players;
use ::rules::vm::{MAX_PICKS, MAX_PICK_ROWS};

/// One player's pick: the roster slot it was made for, the picker's
/// number and the row picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub player: u8,
    pub slot: u8,
    pub row: u8,
}

/// Why bytes are not a pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickError {
    /// The bytes end inside a field.
    Truncated,
    /// Bytes past the third.
    TrailingBytes,
    /// A roster slot outside the sixteen.
    BadPlayer(u8),
    /// A picker number past the two a script may offer.
    BadSlot(u8),
    /// A row past the six a picker holds.
    BadRow(u8),
}

impl std::fmt::Display for PickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pick message error: {self:?}")
    }
}

impl std::error::Error for PickError {}

/// The bytes a pick encodes to.
pub const BYTES: usize = 3;

impl Pick {
    /// The payload: the player, the picker and the row, one byte each.
    pub fn encode(&self) -> [u8; BYTES] {
        [self.player, self.slot, self.row]
    }

    /// Reads [`Pick::encode`]'s bytes and nothing else. The bounds are
    /// the picker's own; the host's `Vm::choose` still refuses a row a
    /// picker does not hold and a player outside its roster.
    pub fn decode(bytes: &[u8]) -> Result<Pick, PickError> {
        let [player, slot, row] = bytes else {
            return Err(match bytes.len() < BYTES {
                true => PickError::Truncated,
                false => PickError::TrailingBytes,
            });
        };
        if *player as usize >= MAX_PLAYERS {
            return Err(PickError::BadPlayer(*player));
        }
        if *slot as usize >= MAX_PICKS {
            return Err(PickError::BadSlot(*slot));
        }
        if *row as usize >= MAX_PICK_ROWS {
            return Err(PickError::BadRow(*row));
        }
        Ok(Pick {
            player: *player,
            slot: *slot,
            row: *row,
        })
    }
}

/// The picks made here whose upstream send has not run yet, oldest
/// first. The transport drains one per tick
/// (`actor_net_link::send_pick`).
static OUTBOX: Mutex<VecDeque<[u8; BYTES]>> = Mutex::new(VecDeque::new());

/// The most picks held for the transport: a round's two players may pick
/// twice each in the worst replanning, and a pick past these is dropped,
/// as the player has picked again by then.
const OUTBOX_MAX: usize = 8;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A press on a row of a picker this machine's local player was offered:
/// queued for the upstream send. The full outbox drops the pick.
pub fn send(pick: Pick) -> bool {
    let mut outbox = lock(&OUTBOX);
    if outbox.len() >= OUTBOX_MAX {
        return false;
    }
    outbox.push_back(pick.encode());
    true
}

/// This machine's id as its local players carry it (the player's machine
/// field, which the game replicates), set by the client's asking and sent
/// with each pick.
static MACHINE: Mutex<Option<[u8; 6]>> = Mutex::new(None);

/// Keeps `machine` as this machine's id for the picks it sends.
pub fn set_local_machine(machine: Option<[u8; 6]>) {
    *lock(&MACHINE) = machine;
}

/// The machine id kept by [`set_local_machine`].
pub fn local_machine() -> Option<[u8; 6]> {
    *lock(&MACHINE)
}

/// The machine id of this machine's first listed local player.
pub fn read_local_machine(m: &dyn GuestMemory) -> Option<[u8; 6]> {
    let array = players::array(m).ok()??;
    let live = array.live(m, 16).ok()?;
    live.into_iter()
        .filter(|(_, p)| p.listed(m).unwrap_or(false))
        .find(|(_, p)| p.local_user(m).is_ok_and(|u| u >= 0))
        .and_then(|(_, p)| p.machine(m).ok())
}

/// The roster slots of the remote, listed players whose machine id is
/// `machine`: the players a peer that named it may pick for.
pub fn seats_of(m: &dyn GuestMemory, machine: [u8; 6]) -> Vec<u8> {
    let Ok(Some(array)) = players::array(m) else {
        return Vec::new();
    };
    let Ok(live) = array.live(m, 16) else {
        return Vec::new();
    };
    live.into_iter()
        .filter(|(_, p)| p.listed(m).unwrap_or(false))
        .filter(|(_, p)| p.local_user(m).is_ok_and(|u| u < 0))
        .filter(|(_, p)| p.machine(m).is_ok_and(|id| id == machine))
        .map(|(handle, _)| handle.index() as u8)
        .collect()
}

/// The queued picks' bytes, in order, taken by the transport.
pub fn take_outbox() -> Vec<[u8; BYTES]> {
    lock(&OUTBOX).drain(..).collect()
}

/// The host takes a pick a peer sent: decoded, for one of the roster
/// slots `seats` says that peer's players sit in, and handed to the same
/// reports a local picker's press makes (`crate::rules::world::pick`),
/// so the VM's `picked` reads it at the next boundary. Anything else is
/// refused with a line.
pub fn receive(bytes: &[u8], seats: &[u8]) -> bool {
    let Ok(pick) = Pick::decode(bytes) else {
        eprintln!("actor-net: pick message {bytes:02x?} refused: not a pick");
        return false;
    };
    if !seats.contains(&pick.player) {
        eprintln!(
            "actor-net: pick for player {} refused: not one of the sender's players",
            pick.player
        );
        return false;
    }
    let accepted = crate::rules::world::pick::report(pick.player, pick.slot, usize::from(pick.row));
    if !accepted {
        eprintln!("actor-net: pick {pick:?} refused: not a row of an offered picker");
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pick_round_trips() {
        let pick = Pick {
            player: 15,
            slot: 1,
            row: 5,
        };
        let bytes = pick.encode();
        assert_eq!(bytes.len(), BYTES);
        assert_eq!(Pick::decode(&bytes), Ok(pick));
        // What decodes encodes to the same bytes.
        assert_eq!(Pick::decode(&bytes).unwrap().encode(), bytes);
    }

    #[test]
    fn the_pick_refuses_each_bad_bound() {
        let good = Pick {
            player: 0,
            slot: 0,
            row: 0,
        }
        .encode();
        // Every truncation and a byte too many.
        for cut in 0..BYTES {
            assert_eq!(Pick::decode(&good[..cut]), Err(PickError::Truncated));
        }
        let mut more = good.to_vec();
        more.extend_from_slice(&good);
        assert_eq!(Pick::decode(&more), Err(PickError::TrailingBytes));
        // A player, picker and row past their bounds.
        let bad = |player, slot, row| {
            Pick::decode(&[player, slot, row]).map(|_| ())
        };
        assert_eq!(bad(MAX_PLAYERS as u8, 0, 0), Err(PickError::BadPlayer(MAX_PLAYERS as u8)));
        assert_eq!(bad(0, MAX_PICKS as u8, 0), Err(PickError::BadSlot(MAX_PICKS as u8)));
        assert_eq!(bad(0, 0, MAX_PICK_ROWS as u8), Err(PickError::BadRow(MAX_PICK_ROWS as u8)));
    }

    /// A host takes a peer's pick into the reports a local picker makes,
    /// and refuses one for a player the peer does not sit. The reports
    /// are drained by the runtime (`rules/world/pick.rs` tests cover the
    /// rest); the tests run side by side, and the report queue is keyed
    /// by thread there, so this one only checks the refusal of the queue
    /// less peer slots.
    #[test]
    fn a_pick_from_a_peer_outside_its_seats_is_refused() {
        let pick = Pick {
            player: 3,
            slot: 0,
            row: 1,
        };
        assert!(!receive(&pick.encode(), &[4, 5]));
    }

    const PLAYERS: u32 = 0x0070_0000;
    const MINE: [u8; 6] = [2, 1, 2, 3, 4, 5];
    const THEIRS: [u8; 6] = [2, 9, 9, 9, 9, 9];

    /// A roster of four: slot 0 local on this machine, 1 and 2 remote on
    /// `THEIRS`, 3 remote on another machine.
    fn roster() -> guest_layout::testing::FakeGuest {
        use guest_layout::view::{fake, players::Player};
        let mut m = guest_layout::testing::FakeGuest::new();
        m.put_u32(guest_layout::points::PLAYERS.base, PLAYERS);
        let players = fake::datums::<Player>(&mut m, PLAYERS, 16);
        for (local, machine) in [(0i16, MINE), (-1, THEIRS), (-1, THEIRS), (-1, [2, 7, 7, 7, 7, 7])] {
            let (_, p) = fake::push(&mut m, &players);
            p.set_local_user(&mut m, local).unwrap();
            m.put(p.addr + guest_layout::view::players::MACHINE, &machine);
        }
        m
    }

    #[test]
    fn a_machine_id_finds_the_remote_players_it_has_and_a_client_reads_its_own() {
        let m = roster();
        assert_eq!(seats_of(&m, THEIRS), vec![1, 2]);
        assert_eq!(seats_of(&m, [2, 7, 7, 7, 7, 7]), vec![3]);
        // The host's own local player is nobody a peer may speak for.
        assert_eq!(seats_of(&m, MINE), Vec::<u8>::new());
        assert_eq!(seats_of(&m, [0; 6]), Vec::<u8>::new());
        assert_eq!(read_local_machine(&m), Some(MINE));
    }

    /// The host's whole path from the tunnel's message to the VM's report:
    /// a pick of a seated player is kept, a pick of another machine's
    /// player is refused.
    #[test]
    fn a_pick_is_kept_for_the_players_of_the_machine_that_sent_it() {
        let m = roster();
        let seats = seats_of(&m, THEIRS);
        let mine = Pick { player: 1, slot: 0, row: 2 };
        let other = Pick { player: 3, slot: 0, row: 1 };
        let _ = crate::rules::world::pick::take();
        assert!(receive(&mine.encode(), &seats));
        assert!(!receive(&other.encode(), &seats));
        assert_eq!(
            crate::rules::world::pick::take(),
            vec![crate::rules::world::pick::Report { player: 1, slot: 0, row: 2 }]
        );
    }
}
