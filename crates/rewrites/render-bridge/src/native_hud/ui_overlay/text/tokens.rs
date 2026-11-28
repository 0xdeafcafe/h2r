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
//! | `0x0022d456` | `0xe409` (`gametype`) | the variant's game type name from the UI tag's second `unic` (`+0x14c`), else the static buffer `0x0055c6e4` |
//! | `0x0022d62b`, `0x0022d655`, `0x0022d67c` | `0xe40b` (`motion_sensor_enabled`), `0xe40c` (`teams_enabled`), `0xe40e` (`weapon_set`) | a string of the UI tag's `unic` (`+0x11c`), by the variant's flags |
//! | `0x0022d6ee` | `0xe429` to `0xe42c` (`player_gamertag`) | the player's gamertag (`0x0054e8e0`, `0xc70` each) |
//! | `0x0022da65` | `0xe42d` to `0xe430` (`hud_scoreboard`) | a string of the HUD's score record |
//! | `0x0022d7a1` | `0xe45c` (`new-content`) | the new-content glyph `0xe007` while `[0x0051055d]` is set |
//! | `0x0022d99e` | `0xe420` (`live_ui_driver_gamertag`) | the Live driver's gamertag at `0x0054e438` |
//! | `0x0022d9e0` | `0xe422` (`hud_quantity`) | the HUD's quantity (`[[0x00510c4c] + 0x1b0]`) |
//! | `0x0022da0d` | `0xe458` (`hud_betraying_player`) | the betrayer's name (the player datum at `[[0x00510c4c] + 0x1b8]`), else `??` |
//! | `0x0022e148`, `0x0022e179`, `0x0022e196`, `0x0022e1b3`, `0x0022e1d0` | `hour_12`, `hour_24`, `minute`, `day`, `month`, `year` | the clock's field (`0x0054e7ce`..) |
//!
//! Any other body panics, named by its token (`ui text: token <name>`).
//! Not ported, each reading a subsystem with no native model: `mapname`
//! (`0x0022d5a7`, the session's map), `hopper_name` and
//! `hopper_description` (`0x0022d52e`, `0x0022d566`), `countdown`
//! (`0x0022d730`), `matchmaking_countdown` (`0x0022d775`),
//! `map_load_percent` (`0x0022d7ce`), `name_of_last_person_to_delay_countdown`
//! (`0x0022d80b`), `leader` (`0x0022d82f`), `network_quality`
//! (`0x0022d85f`), `active_multiplayer_protocol` (`0x0022d93b`),
//! `live_ui_driver_clan_name` (`0x0022d9b4`), `ge_round_time_left`
//! (`0x0022da75`), `target_player_gamertag` (`0x0022daa5`),
//! `game_player_count` (`0x0022dad1`), the coop names (`0x0022db96`,
//! `0x0022dbe0`, `0x0022dc2a`), `storage_blocks` (`0x0022dc68`), the target
//! player's clan and title (`0x0022dfa9`, `0x0022e001`, `0x0022e06c`),
//! `live_ui_driver_clan_level` (`0x0022e0df`), and `player_profile_name`
//! (`0x0022d68f`).

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
/// The session's game variant flags (`+0x48`): bit 0 teams, bit 1 the
/// motion sensor.
const VARIANT_FLAGS: u32 = 0x48;
/// The variant's game type (`+0x44`).
const VARIANT_TYPE: u32 = 0x44;
/// `0x0022d6ee`: the players' records (`0xc70` bytes from `0x0054e8e0`),
/// the gamertag at `+0x470 + 0xc`.
const GAMER_PLAYERS: u32 = 0x0054_e8e0 + 0x470 + 0xc;
const GAMER_STRIDE: u32 = 0xc70;
/// The scoreboard strings of the HUD state's `+0x1bc` record (`0x0013934d`).
const HUD_SCORES: u32 = 0x1bc;
/// The static buffer the game type lookup writes (`0x0055c6e4`), read
/// back whether or not the lookup wrote it.
const GAMETYPE_BUFFER: u32 = 0x0055_c6e4;
/// The tag instance table, the globals tag (`0x004e034c`) and its owner
/// of the screens (`0x004e0350`), the language (`0x0047ff38`).
const TAGS: u32 = 0x004e_3b44;
const GLOBALS: u32 = 0x004e_034c;
const SCREENS: u32 = 0x004e_0350;
const LANGUAGE: u32 = 0x0047_ff38;
/// A locale record in the globals: `+0x188`, `0x1c` bytes per language;
/// its index `+0`, blob `+4`, count `+8`, loaded byte `+0x18`.
const LOCALES: u32 = 0x188;
const LOCALE_BYTES: u32 = 0x1c;
/// What a string the table lacks reads as (`0x0043e5e1`).
const EMPTY_STRING: u32 = 0x0043_e5e1;
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
            let variant = session_variant(mem)?.unwrap_or(DEFAULT_VARIANT);
            Ok(wide(mem, variant.wrapping_add(VARIANT_NAME))?)
        }
        0x0022_d456 => gametype(mem),
        0x0022_d62b => {
            let on = session_variant(mem)?.map(|v| word(mem, v, VARIANT_FLAGS)).transpose()?.is_some_and(|f| f >> 1 & 1 != 0);
            Ok(ui_string(mem, if on { 0x0200_0181 } else { 0x0300_0182 })?)
        }
        0x0022_d655 => {
            let on = session_variant(mem)?.map(|v| word(mem, v, VARIANT_FLAGS)).transpose()?.is_some_and(|f| f & 1 != 0);
            Ok(ui_string(mem, if on { 0x0700_0183 } else { 0x0800_0184 })?)
        }
        0x0022_d67c => Ok(ui_string(mem, 0x0600_00b8)?),
        0x0022_d6ee => {
            let player = match unit {
                0xe429 => 0,
                0xe42a => 1,
                0xe42b => 2,
                _ => 3,
            };
            Ok(narrow(mem, GAMER_PLAYERS.wrapping_add(player * GAMER_STRIDE))?)
        }
        0x0022_da65 => scoreboard(mem, unit),
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

/// A tag's data by its datum (`[0x004e3b44] + (datum & 0xffff) * 0x10 + 8`);
/// 0 for no datum.
fn tag_data(mem: &dyn GuestRead, datum: u32) -> Result<u32, Unread> {
    if datum == u32::MAX {
        return Ok(0);
    }
    word(mem, word(mem, TAGS, 0)?, (datum & 0xffff) * 0x10 + 8)
}

/// `0x001482e8` and `0x00148350`: the globals' UI tag for the screens'
/// mode (`+0x8c`, `+0x94`, `+0x84`), then the tag its datum at `+4` names;
/// 0 when any is absent.
fn ui_globals(mem: &dyn GuestRead) -> Result<u32, Unread> {
    let (screens, globals) = (word(mem, SCREENS, 0)?, word(mem, GLOBALS, 0)?);
    if screens == 0 || globals == 0 || word(mem, globals, 0x110)? == 0 {
        return Ok(0);
    }
    let block = word(mem, globals, 0x114)?;
    if block == 0 {
        return Ok(0);
    }
    let field = match half(mem, screens, 0x10)? as i16 {
        0 => 0x8c,
        1 => 0x94,
        2 => 0x84,
        _ => return Ok(0),
    };
    let tag = tag_data(mem, word(mem, block, field)?)?;
    if tag == 0 {
        return Ok(0);
    }
    tag_data(mem, word(mem, tag, 4)?)
}

/// `0x001a0180` with `0x001a0350`: the string `id` of the `unic` `datum`
/// in the chosen language, as UTF-16 up to `0xff` units. `None` when the
/// lookup writes nothing (no datum, the language's table not loaded).
fn locale_string(mem: &dyn GuestRead, datum: u32, id: u32) -> Result<Option<Vec<u16>>, Unread> {
    if datum == u32::MAX {
        return Ok(None);
    }
    let language = word(mem, LANGUAGE, 0)?;
    if language == u32::MAX {
        // The guest picks one here (`0x0011ca80`).
        return Err(Unread::Unreadable);
    }
    let data = tag_data(mem, datum)?;
    let start = i32::from(half(mem, data, 0x10u32.wrapping_add(language.wrapping_mul(4)))? as i16);
    let count = i32::from(half(mem, data, 0x12u32.wrapping_add(language.wrapping_mul(4)))? as i16);
    let table = word(mem, GLOBALS, 0)?.wrapping_add(LOCALES).wrapping_add(language.wrapping_mul(LOCALE_BYTES));
    if byte(mem, table.wrapping_add(0x18))? == 0 {
        return Ok(None);
    }
    let (index, blob, total) = (word(mem, table, 0)?, word(mem, table, 4)?, word(mem, table, 8)? as i32);
    let mut at = EMPTY_STRING;
    let mut i = start;
    let end = start.wrapping_add(count);
    while i < end {
        if i >= 0 && i <= total.wrapping_sub(1) {
            let entry = index.wrapping_add((i as u32).wrapping_mul(8));
            if word(mem, entry, 0)? == id {
                at = blob.wrapping_add(word(mem, entry, 4)?);
                break;
            }
        }
        i += 1;
    }
    let mut bytes = Vec::new();
    for n in 0..0x400u32 {
        match byte(mem, at.wrapping_add(n))? {
            0 => break,
            b => bytes.push(b),
        }
    }
    let mut units: Vec<u16> = String::from_utf8_lossy(&bytes).encode_utf16().collect();
    units.truncate(MAX_UNITS);
    Ok(Some(units))
}

/// `0x0023620d`: the string `id` of the UI tag's `unic` (`+0x11c`); empty
/// for no tag or no string.
fn ui_string(mem: &dyn GuestRead, id: u32) -> Result<Vec<u16>, Unread> {
    if id == u32::MAX {
        return Ok(Vec::new());
    }
    let ui = ui_globals(mem)?;
    if ui == 0 {
        return Ok(Vec::new());
    }
    let datum = word(mem, ui, 0x11c)?;
    Ok(locale_string(mem, datum, id)?.unwrap_or_default())
}

/// `0x000649c0`: the session variant the UI reads: the session's at
/// `+0x4db0` in a game (mode 3 to 8), else none.
fn session_variant(mem: &dyn GuestRead) -> Result<Option<u32>, Unread> {
    Ok(match session(mem)? {
        Some(session) if (3..=8).contains(&word(mem, session, SESSION_MODE)?) => Some(session.wrapping_add(SESSION_VARIANT)),
        _ => None,
    })
}

/// `0x0022d456`: the name of the variant's game type from the UI tag's
/// second `unic` (`+0x14c`); the static buffer `0x0055c6e4` as it stands
/// when no string is looked up (no UI tag, a type with no name).
fn gametype(mem: &dyn GuestRead) -> Result<Vec<u16>, Gap> {
    let stale = || -> Result<Vec<u16>, Unread> {
        let mut out = Vec::new();
        for i in 0..MAX_UNITS as u32 {
            match half(mem, GAMETYPE_BUFFER, 2 * i)? {
                0 => break,
                u => out.push(u),
            }
        }
        Ok(out)
    };
    let ui = ui_globals(mem)?;
    if ui == 0 {
        return Ok(stale()?);
    }
    let variant = session_variant(mem)?.unwrap_or(DEFAULT_VARIANT);
    let id = match word(mem, variant, VARIANT_TYPE)? {
        0 => 0x0800_010b,
        1 => 0x1000_010d,
        2 => 0x0600_010f,
        3 => 0x0700_0110,
        4 => 0x1000_0111,
        7 => 0x0a00_0113,
        8 => 0x0b00_0115,
        9 => 0x0700_010e,
        _ => return Ok(stale()?),
    };
    match locale_string(mem, word(mem, ui, 0x14c)?, id)? {
        Some(text) => Ok(text),
        None => Ok(stale()?),
    }
}

/// `0x0022da65` (`0x0013934d`): the HUD scoreboard's string for the
/// unit, a wide string in the record at `[HUD + 0x1bc]`; nothing with
/// no record.
fn scoreboard(mem: &dyn GuestRead, unit: u16) -> Result<Vec<u16>, Gap> {
    let record = word(mem, word(mem, HUD_STATE, 0)?, HUD_SCORES)?;
    if record == 0 {
        return Ok(Vec::new());
    }
    let at = match unit {
        0xe42d => 0x1bc,
        0xe42e => 0x208,
        0xe42f => 0x134,
        _ => 0x176,
    };
    Ok(wide(mem, record.wrapping_add(at))?)
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

    fn put_bytes(m: &mut Mem, at: u32, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(4).enumerate() {
            let mut w = [0u8; 4];
            w[..chunk.len()].copy_from_slice(chunk);
            m.0.insert(at + 4 * i as u32, u32::from_le_bytes(w));
        }
    }

    /// A UI tag whose two `unic`s hold one table of strings (`ids`, each
    /// the string after it in `texts`), language 0, loaded.
    fn locale(ids: &[u32], texts: &[&str]) -> Mem {
        let mut m = Mem(BTreeMap::new());
        let (globals, block, tags) = (0x1_0000, 0x1_1000, 0x2_0000);
        m.0.insert(SCREENS, 0x5000);
        m.0.insert(0x5000 + 0x10, 0);
        m.0.insert(GLOBALS, globals);
        m.0.insert(globals + 0x110, 1);
        m.0.insert(globals + 0x114, block);
        m.0.insert(block + 0x8c, 2);
        m.0.insert(TAGS, tags);
        m.0.insert(tags + 2 * 0x10 + 8, 0x2_1000);
        m.0.insert(0x2_1000 + 4, 3);
        m.0.insert(tags + 3 * 0x10 + 8, 0x2_2000);
        m.0.insert(0x2_2000 + 0x11c, 4);
        m.0.insert(0x2_2000 + 0x14c, 5);
        m.0.insert(tags + 4 * 0x10 + 8, 0x2_3000);
        m.0.insert(tags + 5 * 0x10 + 8, 0x2_4000);
        for unic in [0x2_3000, 0x2_4000] {
            m.0.insert(unic + 0x10, ids.len() as u32 * 0x1_0000);
        }
        m.0.insert(LANGUAGE, 0);
        let table = globals + LOCALES;
        m.0.insert(table, 0x3_0000);
        m.0.insert(table + 4, 0x3_1000);
        m.0.insert(table + 8, ids.len() as u32);
        m.0.insert(table + 0x18, 1);
        let mut at = 0;
        for (i, (id, text)) in ids.iter().zip(texts).enumerate() {
            m.0.insert(0x3_0000 + 8 * i as u32, *id);
            m.0.insert(0x3_0000 + 8 * i as u32 + 4, at);
            let mut bytes = text.as_bytes().to_vec();
            bytes.push(0);
            put_bytes(&mut m, 0x3_1000 + at, &bytes);
            at += (bytes.len() as u32).next_multiple_of(4);
        }
        m
    }

    fn in_game(m: &mut Mem, flags: u32) {
        m.0.insert(SESSION_ON & !3, 1);
        m.0.insert(SESSION_STATE, 8);
        m.0.insert(SESSIONS[1], 0x4_0000);
        m.0.insert(0x4_0000 + SESSION_MODE, 4);
        m.0.insert(0x4_0000 + SESSION_VARIANT + VARIANT_FLAGS, flags);
    }

    #[test]
    fn the_variant_option_tokens_look_up_their_string_by_the_flags() {
        let mut m = locale(&[0x0200_0181, 0x0300_0182, 0x0700_0183, 0x0800_0184, 0x0600_00b8], &["Sensor on", "Sensor off", "Teams", "Free for all", "Rockets"]);
        m.0.insert(SESSION_ON & !3, 0);
        assert_eq!(write(&m, 0xe40b, 0x0022_d62b), Ok(text("Sensor off")), "no game: no variant");
        assert_eq!(write(&m, 0xe40c, 0x0022_d655), Ok(text("Free for all")));
        in_game(&mut m, 2);
        assert_eq!(write(&m, 0xe40b, 0x0022_d62b), Ok(text("Sensor on")));
        assert_eq!(write(&m, 0xe40c, 0x0022_d655), Ok(text("Free for all")));
        in_game(&mut m, 1);
        assert_eq!(write(&m, 0xe40b, 0x0022_d62b), Ok(text("Sensor off")));
        assert_eq!(write(&m, 0xe40c, 0x0022_d655), Ok(text("Teams")));
        assert_eq!(write(&m, 0xe40e, 0x0022_d67c), Ok(text("Rockets")));
        m.0.insert(LOCALES + 0x1_0000 + 0x18, 0);
        assert_eq!(write(&m, 0xe40e, 0x0022_d67c), Ok(Vec::new()), "table not loaded");
    }

    #[test]
    fn a_string_the_table_lacks_is_the_empty_one_and_no_tag_writes_nothing() {
        let mut m = locale(&[0x0600_00b8], &["Rockets"]);
        m.0.insert(EMPTY_STRING & !3, 0);
        assert_eq!(write(&m, 0xe40b, 0x0022_d62b), Ok(Vec::new()));
        m.0.insert(SCREENS, 0);
        assert_eq!(write(&m, 0xe40e, 0x0022_d67c), Ok(Vec::new()));
    }

    #[test]
    fn the_game_type_names_its_type_else_the_static_buffer() {
        let mut m = locale(&[0x0700_0110], &["Oddball"]);
        m.0.insert(SESSION_ON & !3, 0);
        m.0.insert(DEFAULT_VARIANT + VARIANT_TYPE, 3);
        put_bytes(&mut m, GAMETYPE_BUFFER, &[b'o', 0, b'l', 0, 0, 0, 0, 0]);
        assert_eq!(write(&m, 0xe409, 0x0022_d456), Ok(text("Oddball")));
        m.0.insert(DEFAULT_VARIANT + VARIANT_TYPE, 5);
        assert_eq!(write(&m, 0xe409, 0x0022_d456), Ok(text("ol")), "type 5 has no name");
        m.0.insert(DEFAULT_VARIANT + VARIANT_TYPE, 12);
        assert_eq!(write(&m, 0xe409, 0x0022_d456), Ok(text("ol")));
        m.0.insert(DEFAULT_VARIANT + VARIANT_TYPE, 3);
        m.0.insert(SCREENS, 0);
        assert_eq!(write(&m, 0xe409, 0x0022_d456), Ok(text("ol")), "no UI tag");
    }

    #[test]
    fn a_gamertag_token_reads_its_players_record_as_narrow_text() {
        let mut m = Mem(BTreeMap::new());
        put_bytes(&mut m, GAMER_PLAYERS, b"One\0");
        put_bytes(&mut m, GAMER_PLAYERS + 3 * GAMER_STRIDE, b"Four\0");
        assert_eq!(write(&m, 0xe429, 0x0022_d6ee), Ok(text("One")));
        assert_eq!(write(&m, 0xe42c, 0x0022_d6ee), Ok(text("Four")));
    }

    #[test]
    fn the_scoreboard_tokens_read_the_records_strings() {
        let mut m = Mem(BTreeMap::new());
        m.0.insert(HUD_STATE, 0x2000);
        m.0.insert(0x2000 + HUD_SCORES, 0);
        assert_eq!(write(&m, 0xe42d, 0x0022_da65), Ok(Vec::new()));
        m.0.insert(0x2000 + HUD_SCORES, 0x3000);
        for (unit, at) in [(0xe42d, 0x1bc), (0xe42e, 0x208), (0xe42f, 0x134), (0xe430, 0x176)] {
            for (i, u) in format!("s{unit:x}").encode_utf16().enumerate() {
                let a = 0x3000 + at + 2 * i as u32;
                *m.0.entry(a & !3).or_insert(0) |= u32::from(u) << ((a & 2) * 8);
            }
            assert_eq!(write(&m, unit, 0x0022_da65), Ok(text(&format!("s{unit:x}"))));
        }
    }
}
