//! The string draw's token handlers (`0x00470204`: a name at `-4`, the
//! unit, the handler), by the body they share. The substitution pass
//! calls one with the unit and a cleared buffer of `0x200` units and
//! replaces the one unit with what it writes (`0x0022d43d`).
//!
//! | Body | Units | Writes |
//! |---|---|---|
//! | `0x0022dcbc` | `0xe446` to `0xe455` (`button_jump` to `button_cancel`), `0xe45e` (`button_dual_fire`) | the glyph of the button the UI user's controller layout binds the action to |
//! | `0x0022df04` | `0xe456`, `0xe457` (`stick_move`, `stick_look`) | the glyph of the stick the layout puts the action on |
//! | `0x0022e1ed` | `0xe459`, `0xe45a` (`animating_left_thumbstick`, `_right_`) | one of the stick's three frames by the UI clock |
//!
//! The other bodies (names, gamertags, Live, the variant, the clock and
//! storage counts) are the game's (`text_token`).

use scene_extract::GuestRead;
use scene_extract::ui::Unread;

use super::super::Gap;
use super::{UI_CLOCK, half, word};

/// The UI's user (`[0x004b9ed8]`), its player through `0x0014de70`: the
/// user table `[0x004e8c20]`, a player at `+0xc` per user.
const UI_USER: u32 = 0x004b_9ed8;
const USERS: u32 = 0x004e_8c20;
/// The player profiles: `[[0x004e8c24] + 0x44]`, `0x21c` bytes each,
/// the controller layout's index at `+0x24` (-1: none).
const PROFILES: u32 = 0x004e_8c24;
const PROFILE_BYTES: u32 = 0x21c;
const PROFILE_LAYOUT: u32 = 0x24;
/// The controller layouts (`0x0051ea18`, `0x1c` bytes each): a byte per
/// action from `+8` (its button), the stick layout `+0x18` (`i16`).
const LAYOUTS: u32 = 0x0051_ea18;
const LAYOUT_BYTES: u32 = 0x1c;
const LAYOUT_BUTTONS: u32 = 8;
const LAYOUT_STICKS: u32 = 0x18;

const BUTTON_FIRST: u16 = 0xe446;
const BUTTON_LAST: u16 = 0xe455;
const DUAL_FIRE: u16 = 0xe45e;
const STICK_MOVE: u16 = 0xe456;
const STICK_LOOK: u16 = 0xe457;
const ANIMATED_LEFT: u16 = 0xe459;
const ANIMATED_RIGHT: u16 = 0xe45a;
/// What a handler writes when it does not know (`swprintf` of
/// `0x00450b30`, and the bodies' own `0x3f`).
const UNKNOWN: u16 = 0x3f;

/// The handler at `handler` for `unit`: what it writes, or the gap that
/// keeps the text the game's.
pub(super) fn write(mem: &dyn GuestRead, unit: u16, handler: u32) -> Result<Vec<u16>, Gap> {
    match handler {
        0x0022_dcbc => button(mem, unit),
        0x0022_df04 => stick(mem, unit),
        0x0022_e1ed => Ok(vec![animated(word(mem, UI_CLOCK, 0)?, unit)]),
        _ => Err(Gap::text("text_token")),
    }
}

/// `0x0014de70` and the profile the UI user's player holds: its
/// controller layout's address, `None` with no user (`Some(None)` when
/// the profile names no layout).
fn layout(mem: &dyn GuestRead) -> Result<Option<Option<u32>>, Unread> {
    let user = word(mem, UI_USER, 0)?;
    if user == u32::MAX {
        return Ok(None);
    }
    let player = word(mem, word(mem, USERS, 0)?, user.wrapping_mul(4).wrapping_add(0xc))?;
    if player == u32::MAX {
        return Ok(None);
    }
    let profiles = word(mem, word(mem, PROFILES, 0)?, 0x44)?;
    let profile = profiles.wrapping_add((player & 0xffff) * PROFILE_BYTES);
    let index = word(mem, profile, PROFILE_LAYOUT)?;
    if index == u32::MAX {
        return Ok(Some(None));
    }
    Ok(Some(Some(LAYOUTS.wrapping_add(index.wrapping_mul(LAYOUT_BYTES)))))
}

/// `0x0022dcbc`: the action's button in the layout, `0xe100` plus it for
/// 0 to 13, an animated stick for 14 and 15 (its first unit), else `?`.
/// Dual fire is the trigger the layout does not fire with: its byte at
/// `+0xf` 7 gives 6, else 7. No user writes `?`; no layout, nothing.
fn button(mem: &dyn GuestRead, unit: u16) -> Result<Vec<u16>, Gap> {
    let at = match unit {
        BUTTON_FIRST..=BUTTON_LAST => Some(u32::from(unit - BUTTON_FIRST)),
        DUAL_FIRE => None,
        _ => return Err(Gap::text("text_token")),
    };
    let layout = match layout(mem)? {
        None => return Ok(vec![UNKNOWN]),
        Some(None) => return Ok(Vec::new()),
        Some(Some(layout)) => layout,
    };
    let byte = |n: u32| -> Result<u8, Unread> {
        let a = layout.wrapping_add(n);
        Ok((word(mem, a & !3, 0)? >> ((a & 3) * 8)) as u8)
    };
    let bound = match at {
        Some(n) => byte(LAYOUT_BUTTONS + n)?,
        None if byte(0xf)? == 7 => 6,
        None => 7,
    };
    let clock = || word(mem, UI_CLOCK, 0);
    Ok(vec![match bound {
        0..=13 => 0xe100 + u16::from(bound),
        14 => animated(clock()?, ANIMATED_LEFT),
        15 => animated(clock()?, ANIMATED_RIGHT),
        _ => UNKNOWN,
    }])
}

/// `0x0022df04`: the layout's sticks (`+0x18`): 0 and 2 put moving on
/// the left stick (`0xe110`) and looking on the right (`0xe111`), 1 and
/// 3 the other way round. No user, no layout or another value: nothing.
fn stick(mem: &dyn GuestRead, unit: u16) -> Result<Vec<u16>, Gap> {
    let Some(Some(layout)) = layout(mem)? else { return Ok(Vec::new()) };
    let sticks = half(mem, layout, LAYOUT_STICKS)? as i16;
    let swapped = match sticks {
        0 | 2 => false,
        1 | 3 => true,
        _ => return Ok(Vec::new()),
    };
    Ok(match (unit, swapped) {
        (STICK_MOVE, false) | (STICK_LOOK, true) => vec![0xe110],
        (STICK_MOVE, true) | (STICK_LOOK, false) => vec![0xe111],
        _ => Vec::new(),
    })
}

/// `0x0022e1ed`: the stick's frame by the UI clock modulo 500: under 166
/// the first, under 333 the second, else the third (left `0xe12c` to
/// `0xe12e`, right `0xe12f` to `0xe131`); another unit is `?`.
fn animated(clock: u32, unit: u16) -> u16 {
    let frame = match clock % 500 {
        0..166 => 0,
        166..333 => 1,
        _ => 2,
    };
    match unit {
        ANIMATED_LEFT => 0xe12c + frame,
        ANIMATED_RIGHT => 0xe12f + frame,
        _ => UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Mem(BTreeMap<u32, u32>);
    impl GuestRead for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    /// User 1 is player 2, whose profile names layout 3: actions 0 to 3
    /// on buttons 5, 14, 15 and 20, dual fire's byte `bytes[7]`, the
    /// sticks `sticks`.
    fn user(fire: u8, sticks: u16) -> Mem {
        let mut m = Mem(BTreeMap::new());
        let layout = LAYOUTS + 3 * LAYOUT_BYTES;
        m.0.insert(UI_USER, 1);
        m.0.insert(USERS, 0x9000);
        m.0.insert(0x9000 + 4 + 0xc, 2);
        m.0.insert(PROFILES, 0xa000);
        m.0.insert(0xa044, 0xb000);
        m.0.insert(0xb000 + 2 * PROFILE_BYTES + PROFILE_LAYOUT, 3);
        m.0.insert(layout + 8, u32::from_le_bytes([5, 14, 15, 20]));
        m.0.insert(layout + 0xc, u32::from(fire) << 24);
        m.0.insert(layout + 0x18, u32::from(sticks));
        m.0.insert(UI_CLOCK, 1200);
        m
    }

    #[test]
    fn a_button_token_is_the_glyph_of_the_button_its_action_is_bound_to() {
        let m = user(0, 0);
        let b = |unit| write(&m, unit, 0x0022_dcbc);
        assert_eq!(b(BUTTON_FIRST), Ok(vec![0xe105]));
        assert_eq!(b(BUTTON_FIRST + 1), Ok(vec![0xe12d]), "14: the left stick's frame at 1200");
        assert_eq!(b(BUTTON_FIRST + 2), Ok(vec![0xe130]));
        assert_eq!(b(BUTTON_FIRST + 3), Ok(vec![UNKNOWN]));
        assert_eq!(b(DUAL_FIRE), Ok(vec![0xe107]));
        assert_eq!(write(&user(7, 0), DUAL_FIRE, 0x0022_dcbc), Ok(vec![0xe106]));
    }

    #[test]
    fn a_button_token_with_no_user_is_a_question_mark_and_with_no_layout_nothing() {
        let mut m = user(0, 0);
        m.0.insert(0xb000 + 2 * PROFILE_BYTES + PROFILE_LAYOUT, u32::MAX);
        assert_eq!(write(&m, BUTTON_FIRST, 0x0022_dcbc), Ok(Vec::new()));
        m.0.insert(UI_USER, u32::MAX);
        assert_eq!(write(&m, BUTTON_FIRST, 0x0022_dcbc), Ok(vec![UNKNOWN]));
    }

    #[test]
    fn the_stick_tokens_follow_the_layouts_sticks() {
        for (sticks, moving, looking) in [(0, 0xe110, 0xe111), (1, 0xe111, 0xe110), (2, 0xe110, 0xe111), (3, 0xe111, 0xe110)] {
            let m = user(0, sticks);
            assert_eq!(write(&m, STICK_MOVE, 0x0022_df04), Ok(vec![moving]));
            assert_eq!(write(&m, STICK_LOOK, 0x0022_df04), Ok(vec![looking]));
        }
        assert_eq!(write(&user(0, 4), STICK_MOVE, 0x0022_df04), Ok(Vec::new()));
    }

    #[test]
    fn an_animated_stick_has_three_frames_a_cycle_of_500_ticks() {
        assert_eq!([0, 165, 166, 332, 333, 499, 500].map(|t| animated(t, ANIMATED_LEFT)), [0xe12c, 0xe12c, 0xe12d, 0xe12d, 0xe12e, 0xe12e, 0xe12c]);
        assert_eq!(animated(400, ANIMATED_RIGHT), 0xe131);
        assert_eq!(animated(0, 0xe000), UNKNOWN);
    }

    #[test]
    fn a_body_not_ported_keeps_the_text_the_games() {
        assert_eq!(write(&user(0, 0), 0xe43d, 0x0022_e0df), Err(Gap::text("text_token")));
    }
}
