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

/// The byte the prologue sets to 1 first.
pub const PROLOGUE_FLAG: u32 = 0x0046_70bc;
/// The shader draw state `0x00016b10` resets.
pub const SHADER_DRAW_STATE: u32 = 0x0048_5b48;
/// The pointer to the view's screen record (`[0x00485890]`): `+0x1c` gets
/// the word at [`SCREEN_WORD`], and `0x000245c0` runs on it.
pub const SCREEN_RECORD: u32 = 0x0048_5890;
pub const SCREEN_WORD: u32 = 0x0048_5600;
/// What `0x000245c0` stores at the record's `+0x18` (EAX at the call).
pub const SCREEN_RECORD_TAG: u32 = 0x0051_ecf0;
/// Image constants the prologue stores (read from the image, never
/// copied): the reset's float (`0x00016b26`) and `0x00024642`'s two.
pub const RESET_FLOAT: u32 = 0x0045_dbd4;
pub const RAMP_FLOATS: [u32; 2] = [0x0045_dc2c, 0x0045_dd30];
/// `0x00024ee0`'s record: used when the pointer `[0x004b54c8]` is set and
/// the byte `[0x004b54d5]` clear, else address 0 (as the game does).
pub const RAMP_RECORD: u32 = 0x004b_5444;
pub const RAMP_GATE: (u32, u32) = (0x004b_54c8, 0x004b_54d5);
/// The word `0x00024ee0` stores at its record's `+0xc`.
pub const RAMP_WORD: u32 = 0x0661_0629;
/// `0x00024ee0`'s three 256-byte tables (each `0x300` bytes apart per
/// set, two sets), in the order it writes them.
pub const RAMP_TABLES: [u32; 3] = [0x004b_9570, 0x004b_9470, 0x004b_9370];

/// What the prologue of `render_screen_effects` leaves (`0x00013870` to
/// `0x000138b3`), for a call skipped in its place: the render states the
/// reset restores (the device's part) and the guest stores, in the
/// game's order.
///
/// With `EBX` 0 at `0x000245c0` the colour ramp `0x00024ee0(0)` computes
/// its curve (`pow`) into its own stack only: the guest tables get the
/// identity bytes, so nothing here needs the curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prologue {
    /// `(state, value)` pairs `0x00018900` sets, in order (none when the
    /// reset has no last pass).
    pub states: Vec<(u32, u32)>,
    /// Guest stores, in order.
    pub writes: Vec<(u32, Vec<u8>)>,
}

/// The byte at `addr` through its aligned word.
fn byte(mem: &dyn GuestRead, addr: u32) -> Option<u8> {
    mem.read_u32(addr & !3).map(|w| (w >> (8 * (addr & 3))) as u8)
}

/// The little-endian word at any address, through aligned words.
fn word(mem: &dyn GuestRead, addr: u32) -> Option<u32> {
    if addr & 3 == 0 {
        return mem.read_u32(addr);
    }
    let mut v = 0u32;
    for i in 0..4 {
        v |= u32::from(byte(mem, addr.wrapping_add(i))?) << (8 * i);
    }
    Some(v)
}

impl Prologue {
    /// Reads what the prologue would do now. `None` when a word it needs
    /// cannot be read.
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let s = SHADER_DRAW_STATE;
        // 0x00016b10: with a last pass, 0x00018900(s, last + 0x11c) sets
        // the list's states (a word: count in bits 9.., first entry in bits
        // 0..9; entries of five bytes, state then value, from the table at
        // `[[s + 0x20] + 0x14]`).
        let last = mem.read_u32(s + 0x24)?;
        let mut states = Vec::new();
        if last != 0 {
            let head = word(mem, last.wrapping_add(0x11c))? & 0xffff;
            let base = mem.read_u32(mem.read_u32(s + 0x20)?.wrapping_add(0x14))?;
            let mut at = (head & 0x1ff).wrapping_mul(5).wrapping_add(base);
            for _ in 0..head >> 9 {
                states.push((u32::from(byte(mem, at)?), word(mem, at.wrapping_add(1))?));
                at = at.wrapping_add(5);
            }
        }
        let reset = mem.read_u32(RESET_FLOAT)?;
        let [ramp_a, ramp_b] = [mem.read_u32(RAMP_FLOATS[0])?, mem.read_u32(RAMP_FLOATS[1])?];
        let screen = mem.read_u32(SCREEN_RECORD)?;
        let screen_word = word(mem, SCREEN_WORD)? as u16 as i16 as i32;
        let (gate_ptr, gate_byte) = (mem.read_u32(RAMP_GATE.0)?, byte(mem, RAMP_GATE.1)?);

        // 0x0001387b, then the reset's stores (0x00016b2e..).
        let mut writes: Vec<(u32, Vec<u8>)> = vec![(PROLOGUE_FLAG, vec![1])];
        let mut put = |addr: u32, v: u32| writes.push((addr, v.to_le_bytes().to_vec()));
        for off in (0x0c..=0x24).step_by(4) {
            put(s + off, 0);
        }
        for off in [0, 0x40] {
            put(s + off, u32::MAX);
        }
        for off in [0x1320, 0x1360, 0x13a0, 0x13e0] {
            put(s + off, u32::MAX);
            put(s + off + 0x10, reset);
        }
        let mut tables = Vec::new();
        if screen != 0 {
            // 0x00013898, then 0x000245c0 with EAX 0x0051ecf0, EBX 0: its
            // stores (0x00024642..), 0x00024ee0(0) with ECX the record, and
            // the count (0x0002468d, `inc` of the 0 stored).
            put(screen.wrapping_add(0x1c), screen_word as u32);
            put(screen.wrapping_add(0x18), SCREEN_RECORD_TAG);
            for (off, v) in [(0, 0), (4, ramp_a), (8, ramp_b), (0xc, 0), (0x10, ramp_a), (0x14, ramp_b), (0x20, 0)] {
                put(screen.wrapping_add(off), v);
            }
            let record = if gate_ptr != 0 && gate_byte == 0 { RAMP_RECORD } else { 0 };
            put(record.wrapping_add(0x10), 0);
            put(record.wrapping_add(0xc), RAMP_WORD);
            let set = screen_word.clamp(0, 1) as u32 * 0x300;
            tables = RAMP_TABLES.iter().map(|&t| t + set).collect();
        }
        for t in tables {
            writes.push((t, (0..=255u8).collect()));
        }
        if screen != 0 {
            writes.push((screen.wrapping_add(0x20), 1u32.to_le_bytes().to_vec()));
        }
        Some(Self { states, writes })
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
