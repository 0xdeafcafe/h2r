//! Host membership and the last accepted action at a film boundary. Guest
//! slot memory supplies the previous action for one-tick fallback on replay.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Roster {
    seats: Vec<(PhantomInfo, bool, action::FilmGather)>,
    hosted_game: Option<u32>,
    #[serde(default)]
    opening_fade: Option<(u32, bool)>,
    next: u32,
    gone: Vec<[u8; 6]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retiring: Vec<(u32, u32)>,
    playerless_host: bool,
    game_only: bool,
    match_only: bool,
}

impl Phantoms {
    pub fn film_roster(&self) -> Roster {
        let b = self.lock();
        Roster {
            seats: b
                .list
                .iter()
                .map(|p| (p.info.clone(), p.made, p.buttons.for_film()))
                .collect(),
            hosted_game: b.hosted_game,
            opening_fade: b.opening_fade,
            next: b.next,
            gone: b.gone.clone(),
            retiring: b.retiring.clone(),
            playerless_host: b.playerless_host,
            game_only: b.game_only,
            match_only: b.match_only,
        }
    }
    pub fn restore_film_roster(&self, r: &Roster) -> Result<(), String> {
        if r.seats.len() > MAX_PHANTOMS || r.gone.iter().any(|&mac| handle_of(mac).is_none()) {
            return Err("invalid phantom roster".into());
        }
        let mut list = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (info, made, buttons) in &r.seats {
            if info.handle.0 >= r.next
                || machine_id(info.handle) != info.machine
                || !seen.insert(info.handle)
            {
                return Err("invalid phantom machine identity".into());
            }
            list.push(Phantom {
                peer_identity: session::PeerIdentity::new(&info.identity.gamertag),
                peer_hint: None,
                info: info.clone(),
                source: std::sync::Arc::new(Mutex::new(Box::new(
                    crate::pad_source::Recorded::new(Vec::new()),
                ))),
                generation: 0,
                made: *made,
                buttons: buttons.restore()?,
                uplink: action::Uplink::new(info.identity.xuid),
                shot_range: None,
                samples: 0,
                waiting: None,
            });
        }
        let mut books = self.lock();
        let maintenance = books.maintenance;
        *books = Books {
            list,
            hosted_game: r.hosted_game,
            opening_fade: r.opening_fade,
            next: r.next,
            gone: r.gone.clone(),
            retiring: r.retiring.clone(),
            playerless_host: r.playerless_host,
            game_only: r.game_only,
            match_only: r.match_only,
            // Replay consumes the recorded retail actions, so there are no
            // pending brain jobs or buffered plans to reinstate.
            thinker: Thinker::default(),
            batch: None,
            lobby_membership: Default::default(),
            maintenance,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_running_peer_pending_removal_and_future_joins() {
        let original = Phantoms::default();
        let add = |p: &Phantoms, xuid| {
            p.add(
                Identity {
                    xuid,
                    gamertag: format!("Bot {xuid}"),
                },
                Some(1),
                Box::new(crate::pad_source::Recorded::new(vec![])),
            )
            .unwrap()
        };
        let a = add(&original, 42);
        let b = add(&original, 43);
        {
            let mut book = original.lock();
            book.hosted_game = Some(0x8000);
            for p in &mut book.list {
                p.made = true
            }
        }
        original.remove(b).unwrap();
        let bytes = serde_json::to_vec(&original.film_roster()).unwrap();
        let replay = Phantoms::default();
        replay
            .restore_film_roster(&serde_json::from_slice(&bytes).unwrap())
            .unwrap();
        assert!(replay.stands_for(machine_id(a)));
        assert_eq!(replay.lock().gone, vec![machine_id(b)]);
        assert_eq!(add(&replay, 44).0, b.0 + 1);
    }
}
