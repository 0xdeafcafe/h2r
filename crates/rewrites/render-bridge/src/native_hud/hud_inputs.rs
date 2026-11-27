//! The HUD's shader function inputs: the callback `0x0022ace7` the HUD
//! shader quad installs (`0x0022a659`: `[0x005021d4]`, with its context at
//! `0x005021d0`) for the shader's animated properties (`0x0001b230` asks
//! it for each property's `input` and `input_a`), read in Rust
//! (docs/graphics/native-hud-overlay.md, "The material in Rust").
//!
//! (Code, `h2 disasm 0x22ace7 100`.) The callback switches on the function
//! name (a string ID) and returns the value at a fixed address, a value of
//! the UI globals, or 1.0 (`[0x0045dbc0]`) for any other name. The names
//! whose value comes from a call (the user's unit state through
//! `0x0014de70`, the UI's values through `0x0014cca0`/`0x0014ccf0`) are not
//! read: [`function_input`] gives `None` for them.

use engine::memory::GuestMemory;

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

/// The value at `at`, as the callback loads it (`movss`).
fn at(mem: &dyn GuestMemory, at: u32) -> Option<f32> {
    mem.u32(at).ok().map(f32::from_bits)
}

/// The callback's value for function `name`; `None` for a name whose
/// value the callback computes by a call.
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
        // The user's unit state (`0x0022acb4`) and the UI's values.
        0x0900_05ce | 0x0d00_05ff | 0x1700_05fa | 0x1700_05fb | 0x1800_05fc | 0x1d00_05fd | 0x1d00_05fe => None,
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
    use engine::testing::FakeGuest;

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
        assert_eq!(function_input(&m, 0x0900_05ce), None, "the unit state is a call");
        assert_eq!(function_input(&m, 0x1d00_05fe), None, "a UI value is a call");
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
