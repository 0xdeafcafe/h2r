//! Players (docs/live/live-gamestate.md "Players", docs/game/bots.md §5.1).

use crate::field::Datum;
use crate::mem::{Fault, Mem};

/// A player's name: UTF-16, 32 code units with the NUL.
pub const NAME_UNITS: usize = 32;

crate::layout! {
    /// One player datum (`0x21c` bytes) in [`crate::global::PLAYERS`].
    pub struct Player: 0x21c {
        /// Datum salt; zero when the slot is free.
        SALT salt @ 0x00: u16,
        /// Bit 1 ([`LEAVING`]): the player is leaving and is not listed.
        FLAGS flags @ 0x02: u16,
        /// The player's machine id: the console's Ethernet address, or a
        /// phantom peer's made-up one.
        MACHINE machine @ 0x14: [u8; 6],
        /// The local controller index (-1 for a remote player).
        CONTROLLER controller @ 0x24: i32,
        /// The local user index (-1 for a remote player); selects one of
        /// [`crate::global::PLAYER_SLOTS`].
        LOCAL_USER local_user @ 0x28: i16,
        /// The unit object the player controls.
        UNIT unit @ 0x2c: Datum,
        /// The name ([`NAME_UNITS`] UTF-16 code units, NUL-terminated
        /// unless full).
        NAME name_units @ 0x44: [u16; 32],
        /// Team index.
        TEAM team @ 0xc0: i8,
        /// Spawn protection: retail sets it to the tick rate at every
        /// spawn (`0x14f68e`) and subtracts one per player update
        /// (`0x15bb4f`).
        SPAWN_GRACE spawn_grace @ 0x194: i16,
        /// Active camouflage ticks left (positive while camouflaged).
        CAMO_TIMER camo_timer @ 0x198: i16,
    }
}

/// [`Player::FLAGS`] bit 1: leaving, not listed.
pub const LEAVING: u16 = 1 << 1;

impl Player {
    /// The name, up to its NUL (a full buffer is taken whole).
    pub fn name<M: Mem + ?Sized>(&self, m: &M) -> Result<String, Fault> {
        let units = self.name_units(m)?;
        let len = units.iter().position(|&u| u == 0).unwrap_or(NAME_UNITS);
        Ok(String::from_utf16_lossy(&units[..len]))
    }
    /// Listed in a roster: live and not leaving.
    pub fn listed<M: Mem + ?Sized>(&self, m: &M) -> Result<bool, Fault> {
        Ok(self.salt(m)? != 0 && self.flags(m)? & LEAVING == 0)
    }
}
