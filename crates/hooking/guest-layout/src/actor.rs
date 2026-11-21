//! The live actor pool created at `0x001dfb24`. Each actor names the
//! unit it drives and the character from the loaded map.

use crate::DatumHandle;

pub const CAPACITY: i32 = 256;

crate::record! {
    /// One slot of [`crate::global::ACTORS`].
    pub struct Actor: 0x888 {
        /// The driven unit. `0x001e0dab` installs its reciprocal actor datum.
        unit @ 0x18: Option<DatumHandle>,
        /// `0x000c6810` copies the actor's team onto the unit.
        team @ 0x24: i16,
        /// `0x00203e84` assigns the squad. The low word is the scenario slot.
        squad @ 0x30: u32,
        /// The character tag datum used by `0x001f6516`.
        character @ 0x54: Option<DatumHandle>,
        /// Animation mode selected by `0x001f6644`. The actor's unit
        /// control update (`0x001e43e7`) hands this string ID to the unit.
        animation_mode @ 0x7fc: u32,
        /// Cower requests a low stance at `0x001ae031`. The movement update
        /// puts the resolved crouch request in bit 0 here (`0x001f3e0b`).
        control_flags @ 0x810: u32,
    }
}

/// Each supported mode's regular and crouched byte must remain distinct.
const CROUCHED: u8 = 0x10;

/// The selector at `0x0010eef0` interprets the requested mode when the
/// biped's animation state updates. Each supported ID has a distinct low
/// byte. Its immediate operand in the loaded code supplies the full ID.
fn mode_operand(code: u8) -> Option<u32> {
    match code {
        0x84 => Some(0x0010_ef65),
        0x85 => Some(0x0010_ef6d),
        0x86 => Some(0x0010_ef1d),
        0x89 => Some(0x0010_ef5e),
        0xcb => Some(0x0010_ef47),
        0x2c => Some(0x0010_f00a),
        0x39 => Some(0x0010_f003),
        0xc9 => Some(0x0010_effc),
        _ => None,
    }
}

/// Decode a pose byte to the unit control's (animation mode, crouch).
/// An unknown byte gives no update. The default mode clears the override.
pub fn animation_mode(
    m: &dyn crate::GuestMemory,
    posture: u8,
) -> Result<Option<(u32, bool)>, crate::Fault> {
    for (code, crouch) in [(posture, false), (posture ^ CROUCHED, true)] {
        if code == u8::MAX {
            return Ok(Some((u32::MAX, crouch)));
        }
        if let Some(at) = mode_operand(code) {
            return Ok(Some((m.u32(at)?, crouch)));
        }
    }
    Ok(None)
}

/// Encode the pose the retail selector can present. Check the whole ID
/// before using its low byte: a map-authored override can share that byte.
pub fn posture(m: &dyn crate::GuestMemory, mode: u32, crouch: bool) -> Result<u8, crate::Fault> {
    let code = mode as u8;
    let bare = match mode_operand(code) {
        Some(at) if m.u32(at)? == mode => code,
        _ => u8::MAX,
    };
    Ok(if crouch { bare ^ CROUCHED } else { bare })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    #[test]
    fn each_mode_has_a_distinct_standing_and_crouching_posture() {
        let mut guest = FakeGuest::new();
        let mut used = std::collections::BTreeSet::new();
        for code in 0..=u8::MAX {
            let mode = if let Some(at) = mode_operand(code) {
                let invented = 0x1234_0000 | u32::from(code);
                guest.put_u32(at, invented);
                invented
            } else if code == u8::MAX {
                u32::MAX
            } else {
                continue;
            };
            for crouch in [false, true] {
                let byte = posture(&guest, mode, crouch).unwrap();
                assert!(used.insert(byte));
                assert_eq!(animation_mode(&guest, byte).unwrap(), Some((mode, crouch)));
            }
        }
        for byte in (0..=u8::MAX).filter(|byte| !used.contains(byte)) {
            assert_eq!(animation_mode(&guest, byte).unwrap(), None);
        }
    }
}
