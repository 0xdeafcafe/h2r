//! Guest-visible local seat management. The XAPI tape provides the actual controller reads.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Roster {
    seats: Vec<Seat>,
    next: u32,
    capacity: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Seat {
    handle: u32,
    name: String,
    wanted: u8,
    port: Option<u8>,
    leaving: bool,
    account: Option<(u64, String)>,
}

impl Local {
    pub fn film_roster(&self) -> Roster {
        let s = self.lock();
        Roster {
            next: s.next,
            capacity: s.capacity,
            seats: s
                .bots
                .iter()
                .map(|b| Seat {
                    handle: b.handle.0,
                    name: b.name.clone(),
                    wanted: b.wanted,
                    port: b.port,
                    leaving: b.leaving,
                    account: b.account.as_ref().map(|a| (a.xuid, a.gamertag.clone())),
                })
                .collect(),
        }
    }
    pub fn restore_film_roster(&self, r: &Roster) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        let mut ports = [false; 4];
        if r.capacity.is_some_and(|n| !(1..=4).contains(&n)) || r.seats.len() > 4 {
            return Err("invalid local bot capacity".into());
        }
        let mut bots = Vec::new();
        for seat in &r.seats {
            if seat.wanted >= 4
                || seat.port.is_some_and(|p| p >= 4)
                || !seen.insert(seat.handle)
                || std::mem::replace(&mut ports[usize::from(seat.wanted)], true)
                || seat.handle >= r.next
            {
                return Err("invalid local film bot seat".into());
            }
            bots.push(Bot {
                handle: BotHandle(seat.handle),
                name: seat.name.clone(),
                wanted: seat.wanted,
                port: seat.port,
                leaving: seat.leaving,
                pad: Default::default(),
                account: seat
                    .account
                    .as_ref()
                    .map(|(id, name)| crate::guest::BotAccount {
                        xuid: *id,
                        gamertag: name.clone(),
                    }),
                source: Box::new(Recorded::new(Vec::new())),
            });
        }
        *self.lock() = State {
            bots,
            next: r.next,
            capacity: r.capacity,
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_keeps_placed_and_leaving_seats_and_next_handle() {
        let original = Local::default();
        let add = |l: &Local, p, name| l.add(p, name, Box::new(Recorded::new(vec![]))).unwrap();
        let a = add(&original, 1, "Alice");
        let b = add(&original, 2, "Bob");
        let mut devices = original.backend();
        devices.placed(HostPadId(ID_BASE + u64::from(a.0)), 1);
        devices.placed(HostPadId(ID_BASE + u64::from(b.0)), 2);
        original
            .set_account(
                a,
                Some(crate::guest::BotAccount {
                    xuid: 42,
                    gamertag: "Alice".into(),
                }),
            )
            .unwrap();
        original.remove(b).unwrap();

        let replay = Local::default();
        let bytes = serde_json::to_vec(&original.film_roster()).unwrap();
        replay
            .restore_film_roster(&serde_json::from_slice(&bytes).unwrap())
            .unwrap();
        assert_eq!(original.leaving(), replay.leaving());
        assert_eq!(original.port_mask(), replay.port_mask());
        assert_eq!(replay.account_on(1).unwrap().xuid, 42);
        assert!(replay.unplug(b));
        assert_eq!(add(&replay, 2, "Carol").0, b.0 + 1);
    }
}
