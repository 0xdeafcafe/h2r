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
//! | `0x0022d82c` | `0xe415`, `0xe40d` (`build_number`, `vehicle_set`) | nothing |
//! | `0x0022d503` | `0xe40a` (`variant`) | the session's game variant's name (`+4`), else the default variant's (`0x0054e4a0`) |
//! | `0x0022d7a1` | `0xe45c` (`new-content`) | the new-content glyph `0xe007` while `[0x0051055d]` is set |
//! | `0x0022d99e` | `0xe420` (`live_ui_driver_gamertag`) | the Live driver's gamertag at `0x0054e438` |
//! | `0x0022d9e0` | `0xe422` (`hud_quantity`) | the HUD's quantity (`[[0x00510c4c] + 0x1b0]`) |
//! | `0x0022da0d` | `0xe458` (`hud_betraying_player`) | the betrayer's name (the player datum at `[[0x00510c4c] + 0x1b8]`), else `??` |
//! | `0x0022e148`, `0x0022e179`, `0x0022e196`, `0x0022e1b3`, `0x0022e1d0` | `hour_12`, `hour_24`, `minute`, `day`, `month`, `year` | the clock's field (`0x0054e7ce`..) |
//!
//! Any other body panics, named by its token (`ui text: token <name>`).

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
/// The session's game variant (`0x000649c0`), or the default one.
const DEFAULT_VARIANT: u32 = 0x0054_e4a0;
const VARIANT_NAME: u32 = 4;
/// `0x000597d0`: the session's state (`[0x00527334]`, read when the byte
/// `[0x00527330]` is set): 1 to 6 the session at `[0x00527364]`, 7 to 9
/// the one at `[0x0052736c]`; it counts when its `+0x741c` is not 0.
const SESSION_ON: u32 = 0x0052_7330;
const SESSION_STATE: u32 = 0x0052_7334;
const SESSIONS: [u32; 2] = [0x0052_7364, 0x0052_736c];
const SESSION_MODE: u32 = 0x741c;
const SESSION_VARIANT: u32 = 0x4db0;
const NEW_CONTENT: u32 = 0x0051_055d;
const LIVE_DRIVER_GAMERTAG: u32 = 0x0054_e438;
/// The HUD's state (`[0x00510c4c]`): the quantity at `+0x1b0`, the
/// betraying player's datum at `+0x1b8`.
const HUD_STATE: u32 = 0x0051_0c4c;
const HUD_QUANTITY: u32 = 0x1b0;
const HUD_BETRAYER: u32 = 0x1b8;
/// The players table (`[0x004e8c24]`, `0x0016bae0`): its count `+0x38`,
/// entry size `+0x24`, entries `+0x44`; a player's name at `+0x44`.
const PLAYERS: u32 = 0x004e_8c24;
const PLAYER_NAME: u32 = 0x44;
/// The clock the time tokens read: year, month, day, hour, minute.
const YEAR: u32 = 0x0054_e7ce;
const MONTH: u32 = 0x0054_e7d0;
const DAY: u32 = 0x0054_e7d4;
const HOUR: u32 = 0x0054_e7d6;
const MINUTE: u32 = 0x0054_e7d8;
/// A handler writes at most this many units (`0x0008cc20`).
const MAX_UNITS: usize = 0xff;

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
        0x0022_d82c => Ok(Vec::new()),
        0x0022_d503 => {
            let variant = match session(mem)? {
                Some(session) if (3..=8).contains(&word(mem, session, SESSION_MODE)?) => session.wrapping_add(SESSION_VARIANT),
                _ => DEFAULT_VARIANT,
            };
            Ok(wide(mem, variant.wrapping_add(VARIANT_NAME))?)
        }
        0x0022_d7a1 => Ok(if byte(mem, NEW_CONTENT)? != 0 { vec![0xe007] } else { Vec::new() }),
        0x0022_d99e => Ok(narrow(mem, LIVE_DRIVER_GAMERTAG)?),
        0x0022_d9e0 => Ok(decimal(word(mem, word(mem, HUD_STATE, 0)?, HUD_QUANTITY)? as i32)),
        0x0022_da0d => betrayer(mem),
        0x0022_e148 => {
            let hour = half(mem, HOUR, 0)?;
            Ok(decimal(i32::from(if unit == 0xe43e && hour > 12 { hour - 12 } else { hour })))
        }
        0x0022_e179 => Ok(format!("{:02}", half(mem, MINUTE, 0)?).encode_utf16().collect()),
        0x0022_e196 => Ok(decimal(i32::from(half(mem, DAY, 0)?))),
        0x0022_e1b3 => Ok(decimal(i32::from(half(mem, MONTH, 0)?))),
        0x0022_e1d0 => Ok(decimal(i32::from(half(mem, YEAR, 0)?))),
        _ => unimplemented!("ui text: token {}", name(unit)),
    }
}

/// The token's name in the string draw's table (`0x00470204`).
pub(super) fn name(unit: u16) -> &'static str {
    match unit {
        0xe408 => "mapname",
        0xe409 => "gametype",
        0xe40b => "motion_sensor_enabled",
        0xe40c => "teams_enabled",
        0xe40e => "weapon_set",
        0xe40f..=0xe412 | 0xe431 => "player_profile_name",
        0xe413 => "countdown",
        0xe414 => "name_of_last_person_to_delay_countdown",
        0xe416 => "leader",
        0xe417..=0xe41e | 0xe432 => "network_quality",
        0xe41f => "active_multiplayer_protocol",
        0xe421 => "live_ui_driver_clan_name",
        0xe423 => "ge_round_time_left",
        0xe424 => "target_player_gamertag",
        0xe425 => "game_player_count",
        0xe426 => "coop_level_name",
        0xe427 => "coop_level_description",
        0xe428 => "coop_difficulty",
        0xe429..=0xe42c => "player_gamertag",
        0xe42d..=0xe430 => "hud_scoreboard",
        0xe433..=0xe436 | 0xe444 | 0xe445 => "storage_blocks",
        0xe437 => "target_player_clan_name",
        0xe438 => "target_player_game_title_name",
        0xe43a => "hopper_name",
        0xe43b => "hopper_description",
        0xe43c => "target_player_clan_level",
        0xe43d => "live_ui_driver_clan_level",
        0xe45b => "matchmaking_countdown",
        0xe45d => "map_load_percent",
        _ => "unknown",
    }
}

fn byte(mem: &dyn GuestRead, at: u32) -> Result<u8, Unread> {
    Ok((word(mem, at & !3, 0)? >> ((at & 3) * 8)) as u8)
}

/// `swprintf` of `%d`.
fn decimal(n: i32) -> Vec<u16> {
    n.to_string().encode_utf16().collect()
}

/// A wide string at `at`, cut at [`MAX_UNITS`] (`0x0008cc20`).
fn wide(mem: &dyn GuestRead, at: u32) -> Result<Vec<u16>, Unread> {
    let mut out = Vec::new();
    for i in 0..MAX_UNITS as u32 {
        match half(mem, at, 2 * i)? {
            0 => break,
            u => out.push(u),
        }
    }
    Ok(out)
}

/// `0x0013fc90` with a count of `0x100`: a byte string widened, a byte
/// past `0x7f` written as `0x25a1`, at most `0xff` units.
fn narrow(mem: &dyn GuestRead, at: u32) -> Result<Vec<u16>, Unread> {
    let mut out = Vec::new();
    for i in 0..MAX_UNITS as u32 {
        match byte(mem, at.wrapping_add(i))? {
            0 => break,
            b if b > 0x7f => out.push(0x25a1),
            b => out.push(u16::from(b)),
        }
    }
    Ok(out)
}

/// `0x000597d0`: the session the UI reads, if any.
fn session(mem: &dyn GuestRead) -> Result<Option<u32>, Unread> {
    if byte(mem, SESSION_ON)? == 0 {
        return Ok(None);
    }
    let at = match word(mem, SESSION_STATE, 0)? {
        1..=6 => SESSIONS[0],
        7..=9 => SESSIONS[1],
        _ => return Ok(None),
    };
    let session = word(mem, at, 0)?;
    Ok((word(mem, session, SESSION_MODE)? != 0).then_some(session))
}

/// `0x0022da0d`: `??`, then the betraying player's name when its datum
/// names a live entry of the players table (`0x0016bae0`: index under
/// the count, the entry's salt its upper half).
fn betrayer(mem: &dyn GuestRead) -> Result<Vec<u16>, Gap> {
    let datum = word(mem, word(mem, HUD_STATE, 0)?, HUD_BETRAYER)?;
    let unknown = vec![UNKNOWN, UNKNOWN];
    if datum == u32::MAX {
        return Ok(unknown);
    }
    let table = word(mem, PLAYERS, 0)?;
    let index = datum & 0xffff;
    if index as i32 >= word(mem, table, 0x38)? as i32 {
        return Ok(unknown);
    }
    let entry = word(mem, table, 0x24)?.wrapping_mul(index).wrapping_add(word(mem, table, 0x44)?);
    let salt = half(mem, entry, 0)?;
    if salt == 0 || i32::from(salt as i16) != (datum as i32) >> 16 {
        return Ok(unknown);
    }
    Ok(wide(mem, entry.wrapping_add(PLAYER_NAME))?)
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
        _ => unimplemented!("ui text: button token {unit:#x}"),
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
    #[should_panic(expected = "ui text: token live_ui_driver_clan_level")]
    fn a_body_not_ported_panics_named() {
        let _ = write(&user(0, 0), 0xe43d, 0x0022_e0df);
    }

    fn put_wide(m: &mut Mem, at: u32, text: &str) {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        units.resize(units.len().next_multiple_of(2), 0);
        for (i, pair) in units.chunks(2).enumerate() {
            m.0.insert(at + 4 * i as u32, u32::from(pair[0]) | u32::from(pair[1]) << 16);
        }
    }

    fn text(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn the_clock_tokens_write_its_fields_twelve_hour_past_noon() {
        let mut m = Mem(BTreeMap::new());
        m.0.insert(YEAR & !3, 2026 << 16);
        m.0.insert(MONTH, 11);
        m.0.insert(DAY, 28 | 15 << 16);
        m.0.insert(MINUTE, 7);
        assert_eq!(write(&m, 0xe43e, 0x0022_e148), Ok(text("3")));
        assert_eq!(write(&m, 0xe43f, 0x0022_e148), Ok(text("15")));
        assert_eq!(write(&m, 0xe440, 0x0022_e179), Ok(text("07")));
        assert_eq!(write(&m, 0xe441, 0x0022_e196), Ok(text("28")));
        assert_eq!(write(&m, 0xe442, 0x0022_e1b3), Ok(text("11")));
        assert_eq!(write(&m, 0xe443, 0x0022_e1d0), Ok(text("2026")));
        m.0.insert(DAY, 28 | 12 << 16);
        assert_eq!(write(&m, 0xe43e, 0x0022_e148), Ok(text("12")), "noon stays 12");
    }

    #[test]
    fn the_variant_is_the_sessions_in_a_game_else_the_default() {
        let mut m = Mem(BTreeMap::new());
        put_wide(&mut m, DEFAULT_VARIANT + VARIANT_NAME, "Slayer");
        m.0.insert(SESSION_ON & !3, 0);
        assert_eq!(write(&m, 0xe40a, 0x0022_d503), Ok(text("Slayer")));
        m.0.insert(SESSION_ON & !3, 1);
        m.0.insert(SESSION_STATE, 8);
        m.0.insert(SESSIONS[1], 0x10_0000);
        m.0.insert(0x10_0000 + SESSION_MODE, 4);
        put_wide(&mut m, 0x10_0000 + SESSION_VARIANT + VARIANT_NAME, "Rockets");
        assert_eq!(write(&m, 0xe40a, 0x0022_d503), Ok(text("Rockets")));
        m.0.insert(0x10_0000 + SESSION_MODE, 2);
        assert_eq!(write(&m, 0xe40a, 0x0022_d503), Ok(text("Slayer")), "mode 2 is no game");
    }

    #[test]
    fn the_hud_tokens_write_the_quantity_and_the_betrayer() {
        let mut m = Mem(BTreeMap::new());
        m.0.insert(HUD_STATE, 0x2000);
        m.0.insert(0x2000 + HUD_QUANTITY, 3);
        m.0.insert(0x2000 + HUD_BETRAYER, u32::MAX);
        assert_eq!(write(&m, 0xe422, 0x0022_d9e0), Ok(text("3")));
        assert_eq!(write(&m, 0xe458, 0x0022_da0d), Ok(text("??")));
        m.0.insert(PLAYERS, 0x3000);
        m.0.insert(0x3000 + 0x38, 4);
        m.0.insert(0x3000 + 0x24, 0x100);
        m.0.insert(0x3000 + 0x44, 0x4000);
        m.0.insert(0x4200, 0xe0ab);
        put_wide(&mut m, 0x4200 + PLAYER_NAME, "Ash");
        m.0.insert(0x2000 + HUD_BETRAYER, 0xe0ab_0002);
        assert_eq!(write(&m, 0xe458, 0x0022_da0d), Ok(text("Ash")));
        m.0.insert(0x2000 + HUD_BETRAYER, 0xe0ac_0002);
        assert_eq!(write(&m, 0xe458, 0x0022_da0d), Ok(text("??")), "a stale salt");
    }

    #[test]
    fn the_small_tokens_write_their_static_state() {
        let mut m = Mem(BTreeMap::new());
        m.0.insert(NEW_CONTENT & !3, 0);
        assert_eq!(write(&m, 0xe45c, 0x0022_d7a1), Ok(Vec::new()));
        m.0.insert(NEW_CONTENT & !3, 1 << 8);
        assert_eq!(write(&m, 0xe45c, 0x0022_d7a1), Ok(vec![0xe007]));
        m.0.insert(LIVE_DRIVER_GAMERTAG, u32::from_le_bytes([b'A', 0xe9, b'z', 0]));
        assert_eq!(write(&m, 0xe420, 0x0022_d99e), Ok(vec![0x41, 0x25a1, 0x7a]));
        assert_eq!(write(&m, 0xe415, 0x0022_d82c), Ok(Vec::new()));
    }
}
