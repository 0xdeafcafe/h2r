//! Keep Phantom seats out of the retail sound owner and listener walks.

use std::sync::{Arc, Mutex};

use guest_layout::view::players;
use guest_layout::{DatumHandle, GuestMemory, MemFault, points};
use guest_layout::sound::SoundListener;
use hooks::{Builder, Flow};

const LOCAL_COUNT: u32 = 0x8;
const LOCAL_MAP: usize = 4; // relative to LOCAL_COUNT
const USERS: usize = 4;
const LOCAL_BYTES: usize = LOCAL_MAP + 4 * USERS;

#[derive(Clone, Copy, Default)]
struct Seat {
    player: Option<DatumHandle>,
    phantom: Option<bool>,
}

impl Seat {
    fn new(player: Option<DatumHandle>, machine: Option<[u8; 6]>) -> Self {
        Self {
            player,
            phantom: machine.map(|id| bots::phantom::handle_of(id).is_some()),
        }
    }

    /// The same local-user index is used for owner lookup and listener
    /// refresh. Keep humans in their original slots, including holes.
    fn sound_player(self) -> Option<DatumHandle> {
        self.player.filter(|_| self.phantom != Some(true))
    }
}

struct LocalUsers {
    addr: u32,
    bytes: [u8; LOCAL_BYTES],
    seats: [Seat; USERS],
}

impl LocalUsers {
    fn read(m: &dyn GuestMemory) -> Result<Option<Self>, MemFault> {
        let globals = m.u32(points::PLAYER_GLOBALS.base)?;
        if globals == 0 {
            return Ok(None);
        }
        let addr = globals + LOCAL_COUNT;
        let mut bytes = [0; LOCAL_BYTES];
        m.read(addr, &mut bytes)?;
        let players = players::array(m)?;
        let mut seats = [Seat::default(); USERS];
        for (slot, raw) in seats.iter_mut().zip(bytes[LOCAL_MAP..].chunks_exact(4)) {
            let datum = DatumHandle::from_raw(u32::from_le_bytes(raw.try_into().unwrap()));
            let machine = if let (Some(players), Some(datum)) = (&players, datum) {
                players.get(m, datum)?.map(|p| p.machine(m)).transpose()?
            } else {
                None
            };
            *slot = Seat::new(datum, machine);
        }
        Ok(Some(Self { addr, bytes, seats }))
    }

    fn sound_map(&self) -> Option<[u8; LOCAL_BYTES]> {
        let mut bytes = self.bytes;
        let mut hidden = 0;
        for (seat, raw) in self.seats.iter().zip(bytes[LOCAL_MAP..].chunks_exact_mut(4)) {
            if seat.sound_player() != seat.player {
                raw.copy_from_slice(&DatumHandle::NONE.raw().to_le_bytes());
                hidden += 1;
            }
        }
        if hidden == 0 {
            return None;
        }
        let count = i16::from_le_bytes(bytes[..2].try_into().unwrap());
        bytes[..2].copy_from_slice(&count.saturating_sub(hidden).max(0).to_le_bytes());
        Some(bytes)
    }

    fn quiet_listeners(&self, m: &mut runtime::Guest) -> Result<(), runtime::GuestFault> {
        let globals = m.u32(points::SOUND_GLOBALS.base)?;
        if globals != 0 {
            for (user, seat) in self.seats.iter().enumerate() {
                let Some(listener) = SoundListener::for_user(globals, user) else {
                    continue;
                };
                let active = listener.addr_of(SoundListener::ACTIVE);
                if seat.sound_player().is_none() && m.u8(active)? != 0 {
                    m.set_u8(active, 0)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn install(b: &mut Builder) {
    // A positioned weapon or object sound can become plain at the owner
    // lookup. Listener refresh also runs from the effect update, outside
    // the sound frame. Each body has its own saved map for nested calls.
    for (point, quiet) in [
        (&points::SOUND_UPDATE, true),
        (&points::SOUND_LISTENERS_UPDATE, true),
        (&points::SOUND_OWNER_USER, false),
    ] {
        let saved = Arc::new(Mutex::new(None::<LocalUsers>));
        let before = Arc::clone(&saved);
        b.raw().before(point, "exclude phantoms from sound", move |c| {
            let mut m = c.ctx.memory();
            let mut saved = super::lock(&before);
            *saved = None;
            if let Ok(Some(users)) = LocalUsers::read(&*m) {
                if quiet {
                    // Retail skips unmapped slots without clearing their active
                    // flag. A departed listener would still win by distance.
                    users.quiet_listeners(&mut m)?;
                }
                if let Some(bytes) = users.sound_map() {
                    m.write(users.addr, bytes)?;
                    *saved = Some(users);
                }
            }
            Ok(Flow::Continue)
        });
        b.raw().after(point, "restore the local-user map after sound", move |c| {
            if let Some(users) = super::lock(&saved).take() {
                c.ctx.memory().write(users.addr, users.bytes)?;
            }
            Ok(())
        });
    }
}

#[cfg(feature = "local-io")]
pub(crate) fn add_agent_method(agent: &agent::Agent) {
    agent.add_game_method(
        agent_proto::method::AUDIO_LISTENERS,
        agent::Access::Read,
        |params, ctx| {
            check_params(&params)?;
            ctx.read_guest(inspect_listeners)?
                .map_err(|_| unavailable())
        },
    );
}

#[cfg(feature = "local-io")]
fn unavailable() -> agent_proto::RpcError {
    agent_proto::RpcError::new(
        agent_proto::code::UNAVAILABLE,
        "game listener state is not initialized or readable",
    )
}

#[cfg(feature = "local-io")]
fn check_params(params: &serde_json::Value) -> Result<(), agent_proto::RpcError> {
    if params.is_null() || params.as_object().is_some_and(|p| p.is_empty()) {
        Ok(())
    } else {
        Err(agent_proto::RpcError::new(
            agent_proto::code::INVALID_PARAMS,
            "audio.listeners takes no params",
        ))
    }
}

#[cfg(feature = "local-io")]
fn inspect_listeners(m: &dyn GuestMemory) -> Result<serde_json::Value, ()> {
    use serde_json::json;

    let globals = m.u32(points::SOUND_GLOBALS.base).map_err(|_| ())?;
    if globals == 0 {
        return Err(());
    }
    let users = LocalUsers::read(m).map_err(|_| ())?.ok_or(())?;
    let mut rows = Vec::new();
    for (user, seat) in users.seats.iter().enumerate() {
        let listener = SoundListener::for_user(globals, user).ok_or(())?;
        if listener.active(m).map_err(|_| ())? != 0 {
            rows.push(json!({
                "user": user,
                "player_datum": seat.player.map(DatumHandle::raw),
                "phantom": seat.phantom,
                "position": listener.position(m).map_err(|_| ())?,
            }));
        }
    }
    Ok(json!({ "listeners": rows }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_human_seats_can_own_a_sound_or_listen() {
        let a = DatumHandle::new(1, 5);
        let b = DatumHandle::new(2, 11);
        let phantom = DatumHandle::new(3, 9);
        let seats = [
            Seat::new(Some(a), Some([0, 1, 2, 3, 4, 5])),
            Seat::new(
                Some(phantom),
                Some(bots::phantom::machine_id(bots::phantom::PhantomHandle(0))),
            ),
            Seat::new(None, None),
            Seat::new(Some(b), Some([0, 1, 2, 3, 4, 5])),
        ];
        let write_players = |players: [Option<DatumHandle>; USERS]| {
            let mut bytes = [0; LOCAL_BYTES];
            for (datum, raw) in players.into_iter().zip(bytes[LOCAL_MAP..].chunks_exact_mut(4)) {
                raw.copy_from_slice(&datum.unwrap_or(DatumHandle::NONE).raw().to_le_bytes());
            }
            bytes
        };
        let mut bytes = write_players([Some(a), Some(phantom), None, Some(b)]);
        bytes[..LOCAL_MAP].copy_from_slice(&[3, 0, 2, 0]);
        let users = LocalUsers {
            addr: 0,
            bytes,
            seats,
        };
        let mut expected = write_players([Some(a), None, None, Some(b)]);
        expected[..LOCAL_MAP].copy_from_slice(&[2, 0, 2, 0]);
        assert_eq!(users.sound_map(), Some(expected));
    }

    #[cfg(feature = "local-io")]
    #[test]
    fn listener_read_reports_even_a_stale_unmapped_listener() {
        use guest_layout::testing::FakeGuest;
        use serde_json::json;

        let mut g = FakeGuest::new();
        let sound = 0x10000;
        let players = 0x11000;
        g.put_u32(points::SOUND_GLOBALS.base, sound);
        g.put_u32(points::PLAYER_GLOBALS.base, players);
        g.put_u32(points::PLAYERS.base, 0);
        g.put(players + LOCAL_COUNT + LOCAL_MAP as u32, &[0xff; 4 * USERS]);
        let stale = SoundListener::for_user(sound, 2).unwrap();
        g.put_u8(stale.addr_of(SoundListener::ACTIVE), 1);
        g.put_f32(stale.addr_of(SoundListener::POSITION), 4.0);

        assert_eq!(
            inspect_listeners(&g).unwrap(),
            json!({
                "listeners": [
                    {"user": 2, "player_datum": null, "phantom": null, "position": [4.0, 0.0, 0.0]}
                ]
            })
        );
        assert!(check_params(&json!({})).is_ok());
        assert_eq!(
            check_params(&json!({"user": 2})).unwrap_err().code,
            agent_proto::code::INVALID_PARAMS
        );
        g.put_u32(points::SOUND_GLOBALS.base, 0);
        assert!(inspect_listeners(&g).is_err());
    }
}
