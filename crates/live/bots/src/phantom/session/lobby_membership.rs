//! Detect changes to a seated custom lobby before rebuilding peer properties and teams.
//! Retail can rewrite an effective profile without publishing a membership revision.

use super::{MAX, at::*};
use guest_layout::GuestMemory;

#[derive(Default)]
pub(in crate::phantom) struct LobbyMembership {
    previous: Vec<u8>,
    sample: Vec<u8>,
    key: Option<Key>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Key {
    session: u32,
    incarnation: u32,
    roster_revision: u64,
    local_peer: u32,
    peer_count: u32,
    player_mask: u32,
    team_game: bool,
}

impl LobbyMembership {
    pub(in crate::phantom) fn invalidate(&mut self) {
        self.key = None;
    }

    /// Two contiguous reads cover machine IDs, names, map readiness, player
    /// slots and both copies of every seated player's configuration. Matchmaking
    /// has an additional party descriptor and uses its own reconciliation path.
    pub(in crate::phantom) fn unchanged(
        &mut self,
        m: &dyn GuestMemory,
        session: u32,
        roster_revision: u64,
    ) -> bool {
        let read = || -> Option<Key> {
            let peer_count = m.u32(session.checked_add(PEER_COUNT)?).ok()?;
            let player_mask = m.u32(session.checked_add(PLAYER_MASK)?).ok()?;
            if peer_count > MAX || player_mask > u32::from(u16::MAX) {
                return None;
            }
            Some(Key {
                session,
                incarnation: m.u32(session.checked_add(INCARNATION)?).ok()?,
                roster_revision,
                local_peer: m.u32(session.checked_add(LOCAL_PEER)?).ok()?,
                peer_count,
                player_mask,
                team_game: super::team_game(m, session).ok()?,
            })
        };
        let Some(key) = read() else {
            self.invalidate();
            return false;
        };
        let peers = (key.peer_count * PEER_SIZE) as usize;
        let slots = u32::BITS - key.player_mask.leading_zeros();
        self.sample.resize(peers + (slots * PLAYER_SIZE) as usize, 0);
        let (peer_bytes, player_bytes) = self.sample.split_at_mut(peers);
        if m.read(session + PEERS, peer_bytes).is_err() || m.read(session + PLAYERS, player_bytes).is_err() {
            self.invalidate();
            return false;
        }
        let same = self.key == Some(key) && self.previous == self.sample;
        self.key = Some(key);
        std::mem::swap(&mut self.previous, &mut self.sample);
        same
    }
}
