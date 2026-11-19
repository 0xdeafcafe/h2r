//! Extensions to the Xbox v4 recording stream used by converted campaigns.
//! Event widths, delays and dispatch remain those of the Xbox reader.
//! A single-weapon recording carries this pair in the two otherwise
//! inapplicable inventory fields. The playback adapter consumes the hints
//! before applying the controls to the unit.
pub const WEAPON_SET: u16 = 0xfffe;
/// The second slot in a single-weapon recording.
pub const SECOND_WEAPON: u8 = 0xfe;

/// Suppress turning toward the recorded facing while continuing to aim.
pub const HOLD_FACING: u32 = 1 << 21;
/// Use the alert pose (combat in the converted animation graph).
pub const FORCE_ALERT: u32 = 1 << 22;
/// Request a seat exit.
pub const EXIT_SEAT: u32 = 1 << 23;
/// Bits private to a single-weapon stream (normally left-hand controls).
pub const HINTS: u32 = HOLD_FACING | FORCE_ALERT | EXIT_SEAT;

/// The mode used by Halo 2's initial unit control at 0xc6f07.
pub const COMBAT: u32 = 0x0600_0086;
/// Halo 2's asleep animation mode.
pub const ASLEEP: u32 = 0x0600_0084;
/// Halo 2's flee animation mode.
pub const FLEE: u32 = 0x0400_0089;
/// Halo 2's flaming animation mode.
pub const FLAMING: u32 = 0x0700_0039;

/// A type-2 event has one signed byte. In a marked stream it holds
/// the low byte of the translated mode ID. The prologue carries the full ID.
pub fn expand_mode(value: u32) -> u32 {
    for id in [ASLEEP, COMBAT, FLEE, FLAMING] {
        if value == (id as i8 as i32) as u32 {
            return id;
        }
    }
    value
}
