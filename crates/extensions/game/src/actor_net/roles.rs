//! The role state a distributed viewer needs for the player's trait query.
//! A role change is a whole mode-state replacement. Keys and digests cover
//! the membership together with the Firefight HUD, even without any actors.

use std::sync::Mutex;

use crate::firefight::hud;

/// An envelope around the optional Firefight mode, distinct from its version.
const WITH_ROLES: u8 = 0x81;
const HEADER: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// The selected variant's script reference, including a script using another type.
    pub prefix: [u8; 8],
    /// One bit for each of the 16 player slots.
    pub mask: u16,
    /// Active camo, extra damage and damage resistance in bits 0 through 2.
    pub traits: u8,
}

static RECEIVED: Mutex<Option<State>> = Mutex::new(None);

/// A copy independent of the client and its packet-receive lock.
pub fn received() -> Option<State> {
    *RECEIVED.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn encode(hud: Vec<u8>, roles: Option<State>) -> Vec<u8> {
    let Some(role) = roles else {
        return hud;
    };
    let mut mode = Vec::with_capacity(HEADER + hud.len());
    mode.push(WITH_ROLES);
    mode.extend_from_slice(&role.prefix);
    mode.extend_from_slice(&role.mask.to_le_bytes());
    mode.push(role.traits);
    mode.extend_from_slice(&hud);
    mode
}

/// A malformed role header clears the answer just like a key with no roles.
fn decode(mode: &[u8]) -> (Option<State>, &[u8]) {
    if mode.first() != Some(&WITH_ROLES) {
        return (None, mode);
    }
    let Some((header, hud)) = mode.split_at_checked(HEADER) else {
        return (None, &[]);
    };
    let mask = u16::from_le_bytes([header[9], header[10]]);
    let traits = header[11];
    if traits & !7 != 0 {
        return (None, &[]);
    }
    let mut prefix = [0; 8];
    prefix.copy_from_slice(&header[1..9]);
    (Some(State { prefix, mask, traits }), hud)
}

/// Publish only the accepted replica's state; an empty blob clears both readers.
pub(super) fn receive(mode: &[u8]) {
    let (role, mode) = decode(mode);
    *RECEIVED.lock().unwrap_or_else(|e| e.into_inner()) = role;
    hud::received().set((!mode.is_empty()).then(|| mode.to_vec()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::actor_net::{Packet, Record};
    use crate::actor_net::client::Client;

    const ROLE: State = State {
        prefix: *b"role ref",
        mask: 1 << 15,
        traits: 0b101,
    };

    #[test]
    fn key_and_delta_replace_the_entire_role_and_hud_on_a_viewer() {
        let mut client = Client::default();
        let hud = vec![3, 5, 8];
        let mode = encode(hud.clone(), Some(ROLE));
        let packet = |tick, record| Packet { tick, records: vec![record] };
        let key = |state| Record::Key { actors: Vec::new(), state };
        assert!(client.receive(&packet(11, key(mode.clone())), 11));
        assert_eq!(decode(client.mode_state()), (Some(ROLE), hud.as_slice()));
        let moved = State { mask: 1, traits: 0b010, ..ROLE };
        assert!(client.receive(&packet(12, Record::State(encode(vec![], Some(moved)))), 12));
        assert_eq!(decode(client.mode_state()), (Some(moved), b"".as_slice()));
        // A delayed key cannot put the role back on its previous holder.
        assert!(!client.receive(&packet(11, key(mode)), 13));
        assert_eq!(decode(client.mode_state()).0, Some(moved));
        // A retail Juggernaut guest can still have an old native role
        // pending release. An explicit empty host mask overrides it.
        let empty = State { mask: 0, ..ROLE };
        let clear = encode(hud.clone(), Some(empty));
        assert!(client.receive(&packet(14, key(clear)), 14));
        assert_eq!(decode(client.mode_state()), (Some(empty), hud.as_slice()));
        assert!(client.receive(&packet(15, key(hud.clone())), 15));
        assert_eq!(decode(client.mode_state()), (None, hud.as_slice()));
    }

    #[test]
    fn the_full_firefight_hud_fits_beside_the_script_role() {
        let hud = hud::Mode {
            stage: hud::Stage::Fighting,
            round: 0,
            rounds: 1,
            wave: 0,
            waves: 1,
            boss: false,
            left: 3,
            skulls: 0,
            lives: hud::Lives::Each((0..hud::MAX_EACH as u8).map(|p| (p, 3)).collect()),
            teams: Some(hud::Teams { scores: [5, 7], second: 0b1010 }),
        };
        let mode = encode(hud.encode(), Some(ROLE));
        assert!(mode.len() <= ::actor_net::wire::MAX_STATE_BYTES);
        let (role, payload) = decode(&mode);
        assert_eq!(role, Some(ROLE));
        assert_eq!(hud::Mode::decode(payload).unwrap(), hud);
    }

    #[test]
    fn incomplete_or_unknown_role_values_give_no_traits() {
        let mode = encode(vec![], Some(ROLE));
        for end in 1..HEADER {
            assert_eq!(decode(&mode[..end]), (None, b"".as_slice()));
        }
        let mut bad = mode;
        bad[11] = 0x80;
        assert_eq!(decode(&bad), (None, b"".as_slice()));
    }
}
