//! The HUD's shader function inputs: the callback `0x0022ace7` the HUD
//! shader quad installs (`0x0022a659`: `[0x005021d4]`, with its context at
//! `0x005021d0`) for the shader's animated properties (`0x0001b230` asks
//! it for each property's `input` and `input_a`), read in Rust
//! (docs/graphics/native-hud-overlay.md, "The material in Rust").
//!
//! (Code, `h2 disasm 0x22ace7 100`.) The callback switches on the function
//! name (a string ID) and returns the value at a fixed address, a value of
//! the UI globals, or 1.0 (`[0x0045dbc0]`) for any other name. Two kinds
//! of name come from a call: the view's user's state (`0x0022acb4`, as
//! `scene_extract::hud::user_state`) and the UI's values ([`ui_value`]).

use guest_layout::memory::GuestMemory;
use scene_extract::hud;

/// The provider the quad passes to `0x0001bd50`: context, then callback.
pub const PROVIDER: u32 = 0x0050_21d0;
pub const CALLBACK: u32 = 0x0022_ace7;

/// The four widget inputs (`hud_input1`..`4`) the widget helper writes
/// before each quad (`0x0022b380`).
pub const HUD_INPUTS: u32 = 0x004e_6980;
/// The value of a name the callback does not match.
const DEFAULT_VALUE: u32 = 0x0045_dbc0;
/// The UI globals pointer and the short `0x11000600` scales.
const UI_GLOBALS: u32 = 0x0051_0c4c;
const UI_SHORT: u32 = 0x1d0;
const UI_SHORT_SCALE: u32 = 0x0045_de4c;
/// The user the view is drawn for.
const VIEW_USER: u32 = 0x004b_9ed8;
/// The UI's player (`-1`: none) and its own values.
const UI_PLAYER: u32 = 0x1b4;
const UI_VALUES: u32 = 0x1c0;
/// The players' table, a player's size and its values.
const PLAYERS: u32 = 0x004e_8c24;
const PLAYER_SIZE: u32 = 0x21c;
const PLAYER_VALUES: u32 = 0x84;
/// The scales of the two byte values, and the value of a clear flag.
const BYTE_SCALES: [u32; 2] = [0x0045_de4c, 0x0045_de50];
const FLAG_CLEAR: u32 = 0x0045_dbcc;

/// The value at `at`, as the callback loads it (`movss`).
fn at(mem: &dyn GuestMemory, at: u32) -> Option<f32> {
    mem.u32(at).ok().map(f32::from_bits)
}

/// A UI value (`0x0022adef`): `0x0014ccf0` on the UI's player's values
/// (`0x0014cca0`), or on the UI's own with no player.
fn ui_value(mem: &dyn GuestMemory, name: u32) -> Option<f32> {
    let globals = mem.u32(UI_GLOBALS).ok().filter(|&g| g != 0)?;
    let values = match mem.u32(globals.checked_add(UI_PLAYER)?).ok()? {
        u32::MAX => globals.checked_add(UI_VALUES)?,
        player => {
            let table = mem.u32(mem.u32(PLAYERS).ok()?.checked_add(0x44)?).ok()?;
            table.checked_add((player & 0xffff) * PLAYER_SIZE)?.checked_add(PLAYER_VALUES)?
        }
    };
    let byte = |n: u32| mem.u8(values.checked_add(n)?).ok();
    let flag = |bit: u8| Some(byte(7)? & bit != 0);
    match name {
        0x0d00_05ff => at(mem, DEFAULT_VALUE),
        0x1700_05fa => Some(f32::from(byte(5)?) * at(mem, BYTE_SCALES[0])?),
        0x1700_05fb => Some(f32::from(byte(6)?) * at(mem, BYTE_SCALES[1])?),
        0x1800_05fc => if flag(1)? { Some(0.0) } else { at(mem, DEFAULT_VALUE) },
        0x1d00_05fd => at(mem, if flag(2)? { DEFAULT_VALUE } else { FLAG_CLEAR }),
        0x1d00_05fe => at(mem, if flag(4)? { DEFAULT_VALUE } else { FLAG_CLEAR }),
        _ => Some(0.0),
    }
}

/// The callback's value for function `name`; `None` when it is not read.
pub fn function_input(mem: &dyn GuestMemory, name: u32) -> Option<f32> {
    match name {
        0x0a00_05cf..=0x0a00_05d2 => at(mem, HUD_INPUTS + 4 * (name - 0x0a00_05cf)),
        0x0d00_05ca => at(mem, 0x004e_6968),
        0x0c00_05cb => at(mem, 0x004e_696c),
        0x0c00_05cc => at(mem, 0x004e_6970),
        0x0e00_05cd => at(mem, 0x004e_6974),
        0x1100_0600 => {
            // `movsx`, `cvtsi2ss`, `mulss` (`0x0022adb6`).
            let globals = mem.u32(UI_GLOBALS).ok().filter(|&g| g != 0)?;
            let short = mem.u16(globals.checked_add(UI_SHORT)?).ok()? as i16;
            Some(f32::from(short) * at(mem, UI_SHORT_SCALE)?)
        }
        // `0x0022ad6b`: 1.0 in the view's user's state, or 0.
        0x0900_05ce => match hud::user_state(mem, mem.u32(VIEW_USER).ok()?)? {
            true => at(mem, DEFAULT_VALUE),
            false => Some(0.0),
        },
        0x0d00_05ff | 0x1700_05fa | 0x1700_05fb | 0x1800_05fc | 0x1d00_05fd | 0x1d00_05fe => ui_value(mem, name),
        _ => at(mem, DEFAULT_VALUE),
    }
}

/// Whether the HUD's provider is the one the quad installs: the setup
/// `0x0001b230` reads goes through it only then.
pub fn provider_installed(mem: &dyn GuestMemory) -> bool {
    mem.u32(PROVIDER + 4).ok() == Some(CALLBACK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_callback_reads_each_named_value_and_one_for_the_rest() {
        let mut m = FakeGuest::new();
        m.put_f32(DEFAULT_VALUE, 1.0);
        for (i, v) in [0.25f32, 0.5, 0.75, 2.0].into_iter().enumerate() {
            m.put_f32(HUD_INPUTS + 4 * i as u32, v);
        }
        m.put_f32(0x004e_6968, 3.0).put_f32(0x004e_696c, 4.0).put_f32(0x004e_6970, 5.0).put_f32(0x004e_6974, 6.0);
        assert_eq!(function_input(&m, 0x0a00_05cf), Some(0.25));
        assert_eq!(function_input(&m, 0x0a00_05d2), Some(2.0));
        assert_eq!(function_input(&m, 0x0d00_05ca), Some(3.0));
        assert_eq!(function_input(&m, 0x0c00_05cb), Some(4.0));
        assert_eq!(function_input(&m, 0x0c00_05cc), Some(5.0));
        assert_eq!(function_input(&m, 0x0e00_05cd), Some(6.0));
        assert_eq!(function_input(&m, 0), Some(1.0), "no name");
        assert_eq!(function_input(&m, 0x0a00_05d3), Some(1.0), "past the four inputs");
    }

    #[test]
    fn the_user_state_name_is_one_in_the_state_and_zero_out_of_it() {
        let mut m = FakeGuest::new();
        m.put_f32(DEFAULT_VALUE, 1.0);
        assert_eq!(function_input(&m, 0x0900_05ce), None, "no view user read");
        // No user: no player, so out of the state.
        m.put_u32(VIEW_USER, u32::MAX);
        assert_eq!(function_input(&m, 0x0900_05ce), Some(0.0));
        // User 1's player is slot 2, whose state byte is 3.
        m.put_u32(VIEW_USER, 1).put_u32(0x004e_8c20, 0x5000).put_u32(0x5000 + 0xc + 4, 0xec70_0002);
        m.put_u32(PLAYERS, 0x8000).put_u32(0x8000 + 0x44, 0x9000);
        let player = 0x9000 + 2 * PLAYER_SIZE;
        m.put(player, &[0u8; 0x90]);
        m.put_u8(player + 0x88, 3);
        assert_eq!(function_input(&m, 0x0900_05ce), Some(1.0));
        m.put_u8(player + 0x88, 2);
        assert_eq!(function_input(&m, 0x0900_05ce), Some(0.0));
    }

    #[test]
    fn a_ui_value_is_the_ui_players_or_the_uis_own() {
        let mut m = FakeGuest::new();
        m.put_f32(DEFAULT_VALUE, 1.0).put_f32(FLAG_CLEAR, 0.25).put_f32(BYTE_SCALES[0], 0.5).put_f32(BYTE_SCALES[1], 0.125);
        m.put(0x7000, &[0u8; 0x200]);
        m.put_u32(UI_GLOBALS, 0x7000).put_u32(0x7000 + UI_PLAYER, u32::MAX);
        // The UI's own: bytes 5 and 6, and flag 2 of byte 7.
        m.put_u8(0x7000 + UI_VALUES + 5, 6).put_u8(0x7000 + UI_VALUES + 6, 8).put_u8(0x7000 + UI_VALUES + 7, 2);
        assert_eq!(function_input(&m, 0x0d00_05ff), Some(1.0));
        assert_eq!(function_input(&m, 0x1700_05fa), Some(3.0));
        assert_eq!(function_input(&m, 0x1700_05fb), Some(1.0));
        assert_eq!(function_input(&m, 0x1800_05fc), Some(1.0), "flag 1 clear");
        assert_eq!(function_input(&m, 0x1d00_05fd), Some(1.0), "flag 2 set");
        assert_eq!(function_input(&m, 0x1d00_05fe), Some(0.25), "flag 4 clear");
        // The UI's player, slot 1: its values at +0x84.
        m.put_u32(0x7000 + UI_PLAYER, 0xec70_0001).put_u32(PLAYERS, 0x8000).put_u32(0x8000 + 0x44, 0x9000);
        let values = 0x9000 + PLAYER_SIZE + PLAYER_VALUES;
        m.put(values, &[0u8; 8]);
        m.put_u8(values + 5, 2).put_u8(values + 7, 1 | 4);
        assert_eq!(function_input(&m, 0x1700_05fa), Some(1.0));
        assert_eq!(function_input(&m, 0x1800_05fc), Some(0.0), "flag 1 set");
        assert_eq!(function_input(&m, 0x1d00_05fd), Some(0.25));
        assert_eq!(function_input(&m, 0x1d00_05fe), Some(1.0));
        // With no UI globals nothing is read.
        m.put_u32(UI_GLOBALS, 0);
        assert_eq!(function_input(&m, 0x1700_05fa), None);
    }

    #[test]
    fn the_ui_short_is_scaled() {
        let mut m = FakeGuest::new();
        m.put_u32(UI_GLOBALS, 0x7000).put_u16(0x7000 + UI_SHORT, (-32i16) as u16).put_f32(UI_SHORT_SCALE, 0.015625);
        assert_eq!(function_input(&m, 0x1100_0600), Some(-0.5));
        m.put_u32(UI_GLOBALS, 0);
        assert_eq!(function_input(&m, 0x1100_0600), None);
    }

    #[test]
    fn the_provider_is_the_huds_only_once_the_quad_installs_it() {
        let mut m = FakeGuest::new();
        assert!(!provider_installed(&m));
        m.put_u32(PROVIDER + 4, CALLBACK);
        assert!(provider_installed(&m));
    }
}
