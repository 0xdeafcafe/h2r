//! Whether the retail multiplayer round takes input, from the game engine.
//! The opening fade is [`super::match_opening_fading`]'s.
use guest_layout::{memory::GuestMemory, points};

use super::R;

const STATE: u32 = 0x6c;
const REQUESTED_STATE: u32 = 0xc04;
const PLAYING: u16 = 1;

// The action consumer skips unit input while player_enable_input(false)
// is in force (0x14d765, written by 0x2a8f16).
const INPUT_DISABLED: u32 = 6;

/// The round is playing, is not ending, and unit input is enabled. Between
/// rounds and while a round ends or starts, retail ignores unit input.
pub(crate) fn playing(m: &dyn GuestMemory) -> R<bool> {
    let engine = m.u32(points::GAME_ENGINE.base)?;
    if engine == 0
        || m.u16(engine + STATE)? != PLAYING
        || m.u32(engine + REQUESTED_STATE)? != u32::from(PLAYING)
    {
        return Ok(false);
    }
    let locals = m.u32(points::PLAYER_GLOBALS.base)?;
    Ok(locals != 0 && m.u8(locals + INPUT_DISABLED)? == 0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    /// A round that is playing, on a host with no local view.
    pub(crate) fn live_round(m: &mut FakeGuest, engine: u32, locals: u32) {
        m.map(engine, 0xc9c)
            .map(locals, 0x20)
            .put_u32(points::GAME_ENGINE.base, engine)
            .put_u32(points::PLAYER_GLOBALS.base, locals)
            .put_u16(engine + STATE, PLAYING)
            .put_u32(engine + REQUESTED_STATE, u32::from(PLAYING));
        for user in 0..crate::guest::LOCAL_USER_COUNT {
            m.put_u32(locals + crate::guest::LOCAL_MAP + 4 * user, u32::MAX);
        }
    }

    /// The round ends or has not started: state 0.
    pub(crate) fn close(m: &mut FakeGuest, engine: u32) {
        m.put_u16(engine + STATE, 0);
    }

    pub(crate) fn open(m: &mut FakeGuest, engine: u32) {
        m.put_u16(engine + STATE, PLAYING);
    }

    #[test]
    fn a_round_takes_input_only_while_playing_and_enabled() {
        const ENGINE: u32 = 0x0068_0000;
        const LOCALS: u32 = 0x0068_1000;
        let mut m = FakeGuest::new();
        m.put_u32(points::GAME_ENGINE.base, 0);
        assert!(!playing(&m).unwrap(), "no engine");
        live_round(&mut m, ENGINE, LOCALS);
        assert!(playing(&m).unwrap());
        close(&mut m, ENGINE);
        assert!(!playing(&m).unwrap(), "state 0");
        open(&mut m, ENGINE);
        m.put_u32(ENGINE + REQUESTED_STATE, 2);
        assert!(!playing(&m).unwrap(), "round ending");
        m.put_u32(ENGINE + REQUESTED_STATE, 1)
            .put_u8(LOCALS + INPUT_DISABLED, 1);
        assert!(!playing(&m).unwrap(), "input disabled");
        m.put_u8(LOCALS + INPUT_DISABLED, 0);
        assert!(playing(&m).unwrap());
        m.put_u32(points::PLAYER_GLOBALS.base, 0);
        assert!(!playing(&m).unwrap(), "no player globals");
    }
}
