//! Stage 5: the view's post passes (docs/graphics/native-renderer.md,
//! "Stage 5: post and HUD"). What one call of `render_screen_effects`
//! (`0x00013870`) would draw, read from guest memory at its entry, so the
//! host can say whether it draws it natively.
//!
//! Research points (Code, `h2 disasm`):
//!
//! - `render_scene_passes` ends with layer 17, `0x00020770` and
//!   `0x00013870(late, colour)` (stack bytes, `ret 8`): `late` is the byte
//!   that also gates `0x0002dd30`, `colour` the one that gates layer 17.
//! - `0x00013870` resets the shader draw state (`0x00016b10` on
//!   `0x00485b48`), sets `[0x004670bc]`, runs `0x000245c0` on `0x0051ecf0`
//!   and, with `colour`, draws the colour pass `0x000214f0` over the view's
//!   target (`[0x004858b8]` bound as a texture, `0x00014f60`, a combiner it
//!   writes into the PSDEF at `0x00484f68`). Then, with a scenario
//!   (`[0x004e0350]`), a view kind (`[0x00485898]`) outside 5..7, the bytes
//!   `[0x00485607]` and `[0x004850c8]` clear and `0x00022710` true, each
//!   entry of the scenario block at `+0x3d0` (count) / `+0x3d4` (address),
//!   `0x24` bytes each, `+0x14` a tag datum and `+0x20`/`+0x22` two
//!   function indices, whose values (`0x00134fc0`) decide whether
//!   `0x0002d140` draws it (S: the scenario's screen effects; 03a's map
//!   holds one `egor`, `effects\screen_effects\blur`). With `late`,
//!   `0x0002df00` last.

use crate::camera::GuestRead;

/// `render_screen_effects`.
pub const RENDER_SCREEN_EFFECTS: u32 = 0x0001_3870;
/// Its stack cleanup (`ret 8`).
pub const SCREEN_EFFECTS_CLEANUP: u32 = 8;
/// The scenario pointer.
pub const SCENARIO: u32 = 0x004e_0350;
/// The scenario block the screen-effect loop walks: count, then address.
pub const SCENARIO_SCREEN_EFFECTS: u32 = 0x3d0;
/// Bytes per entry of that block.
pub const SCREEN_EFFECT_SIZE: u32 = 0x24;
/// The view kind (`0x00015680` takes it; 5 to 7 skip the screen effects).
pub const VIEW_KIND: u32 = 0x0048_5898;
/// Bytes that, set, skip the screen effects.
pub const SCREEN_EFFECTS_OFF: [u32; 2] = [0x0048_5607, 0x0048_50c8];
/// Most screen-effect entries read.
pub const MAX_SCREEN_EFFECTS: u32 = 256;

/// One call of `render_screen_effects`: what it was asked to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostFrame {
    /// The colour pass over the view's target (`0x000214f0`).
    pub colour: bool,
    /// `0x0002df00` at the end.
    pub late: bool,
    /// The view kind (`[0x00485898]`).
    pub view_kind: u32,
    /// The scenario's screen-effect entries (0 without a scenario or past
    /// the reading's bounds); whether each draws is decided by its
    /// functions, not read here.
    pub screen_effects: u32,
    /// The screen-effect loop's gates other than `0x00022710` (not traced)
    /// pass: a scenario, a view kind outside 5..7, both bytes clear.
    pub screen_effects_gated: bool,
}

impl PostFrame {
    /// The frame at the call: `late` and `colour` its stack bytes. `None`
    /// when a word it reads cannot be read.
    pub fn read(mem: &dyn GuestRead, late: bool, colour: bool) -> Option<Self> {
        let view_kind = mem.read_u32(VIEW_KIND)?;
        let scenario = mem.read_u32(SCENARIO)?;
        let screen_effects = if scenario == 0 {
            0
        } else {
            mem.read_u32(scenario.wrapping_add(SCENARIO_SCREEN_EFFECTS))?.min(MAX_SCREEN_EFFECTS)
        };
        let byte = |a: u32| mem.read_u32(a & !3).map(|w| (w >> (8 * (a & 3))) & 0xff);
        let off = SCREEN_EFFECTS_OFF.iter().map(|&a| byte(a)).collect::<Option<Vec<_>>>()?;
        let screen_effects_gated = scenario != 0 && !(5..=7).contains(&view_kind) && off.iter().all(|&b| b == 0);
        Some(Self {
            colour,
            late,
            view_kind,
            screen_effects,
            screen_effects_gated,
        })
    }

    /// Whether the call may draw anything at all.
    pub fn draws(&self) -> bool {
        self.colour || self.late || (self.screen_effects_gated && self.screen_effects > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    #[test]
    fn the_frame_reads_its_gates() {
        let mut m = Mem::default();
        m.w(VIEW_KIND, 1);
        m.w(SCENARIO, 0x0070_0000);
        m.w(0x0070_0000 + SCENARIO_SCREEN_EFFECTS, 2);
        m.w(0x0048_5604, 0);
        m.w(0x0048_50c8, 0);
        let f = PostFrame::read(&m, false, true).expect("reads");
        assert_eq!((f.colour, f.late, f.view_kind, f.screen_effects, f.screen_effects_gated), (true, false, 1, 2, true));
        assert!(f.draws());
        // Byte 0x00485607 is the top byte of its word.
        m.w(0x0048_5604, 0x0100_0000);
        let f = PostFrame::read(&m, false, false).unwrap();
        assert!(!f.screen_effects_gated && !f.draws());
        m.w(0x0048_5604, 0x00ff_ffff);
        assert!(PostFrame::read(&m, false, false).unwrap().screen_effects_gated, "other bytes of the word");
        // View kinds 5 to 7 skip them; no scenario has none.
        m.w(VIEW_KIND, 6);
        assert!(!PostFrame::read(&m, true, false).unwrap().screen_effects_gated);
        m.w(VIEW_KIND, 8);
        m.w(SCENARIO, 0);
        let f = PostFrame::read(&m, true, false).unwrap();
        assert_eq!((f.screen_effects, f.screen_effects_gated, f.draws()), (0, false, true));
        // A huge count is bounded; an unreadable word gives nothing.
        m.w(SCENARIO, 0x0070_0000);
        m.w(0x0070_0000 + SCENARIO_SCREEN_EFFECTS, u32::MAX);
        assert_eq!(PostFrame::read(&m, false, false).unwrap().screen_effects, MAX_SCREEN_EFFECTS);
        m.0.remove(&VIEW_KIND);
        assert_eq!(PostFrame::read(&m, false, false), None);
    }
}
