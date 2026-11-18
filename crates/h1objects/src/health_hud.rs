//! Halo 1's health meter in the converted player's Halo 2 HUD
//! (docs/game/health.md, "HUD").
//!
//! A converted Halo 1 map runs Halo 1's health model (`boot::health`), so
//! its player needs to see the body's health. Halo 2's HUD already reads it:
//! a bitmap widget's input 17 is the unit's body vitality (`0x13ad48`,
//! case `0x11`: HUD state `+0x28`, which `0x13992a` fills from the body
//! `+0xec`). So the meter is a widget in the player's `nhdt`, built here:
//!
//! - a copy of the shield meter's widget (the one with special type 5), so
//!   it has the shield meter's anchor, state flags, bitmap, sequences and
//!   per-screen-mode offsets: whatever hides the shield meter hides it
//!   (the whole HUD gate `0x13939b`, which Blind, cinematics and
//!   `show_hud` act on, and the widget's own state gate `0x13ac87`), and
//!   it scales and moves with split screen like the shield meter;
//! - input 0 set to 17 (body) and the special type set to 0 (type 5 is the
//!   shield code path in `0x22aff1`, which reads the shield, not the input);
//! - raised [`RAISE`] pixels per screen mode, so it sits as a second arc
//!   above the shield's;
//! - drawn with its own shader: a copy of `ui\hud\shaders\ammo_meter`
//!   (`widget_legacy_meter` template: fills the bitmap where its gradient is
//!   below `hud_input1`, with fixed colours), whose fill function is given
//!   the shield shader's output range so the value covers the same arc.
//!
//! The converted player biped (any Halo 1 biped named like `cyborg`, which
//! is what both converters make the player) points its HUD interfaces
//! (`unit +0x1a0`) at the new `nhdt`. Every other unit keeps its template's.

use map::TagId;
use map::build::Block;

/// `unit +0x1a0`: the unit's HUD interfaces, a block of 8-byte `nhdt`
/// references (D: `masterchief` and `masterchief_mp` have 2, both
/// `ui\hud\masterchief`).
pub const UNIT_HUD_INTERFACES: usize = 0x1a0;
pub const HUD_INTERFACE_SIZE: usize = 8;

/// `nhdt` root size, and its bitmap widget block (`0x13a532` walks `+0x08`
/// with stride `0x64`; text widgets `+0x10`, `0x54`; effects `+0x20`, `0x50`).
pub const NHDT_ROOT: usize = 0x28;
pub const NHDT_BITMAP_WIDGETS: usize = 0x08;
pub const BITMAP_WIDGET_SIZE: usize = 0x64;

/// Bitmap widget fields (E: `0x13a532`, `0x13ac87`, `0x13ad48`, `0x22aff1`).
pub mod widget {
    /// The four input bytes, turned into floats by `0x13ad48`.
    pub const INPUTS: usize = 0x04;
    /// State flags tested by `0x13ac87` (`+0x08`..`+0x1b`).
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    pub const BITMAP: usize = 0x20;
    pub const SHADER: usize = 0x28;
    /// Offsets (two `i16`, x then y) for fullscreen, half and quarter screen
    /// (`0x22b128`, `0x22b0da`, `0x22b089`).
    pub const OFFSETS: [usize; 3] = [0x34, 0x38, 0x3c];
    /// The widget's effect block (count, address).
    pub const EFFECTS: usize = 0x58;
    /// Special type (`u16`, `0x22b2d0`): 5 is the shield meter.
    pub const SPECIAL: usize = 0x60;
}

/// `0x13ad48`: input 16 is the unit's shield (HUD state `+0x2c`), 17 its
/// body (`+0x28`).
pub const INPUT_SHIELD: u8 = 16;
pub const INPUT_BODY: u8 = 17;
/// The special type `0x22aff1` draws as the shield meter.
pub const SPECIAL_SHIELD_METER: u16 = 5;

/// How far above the shield arc the health arc sits, in HUD pixels, for
/// fullscreen, half and quarter screen. Placement is to be confirmed in a
/// game run (docs/game/health.md, "Only a game run can show").
pub const RAISE: [i16; 3] = [10, 8, 8];

/// `shad +0x20`: the compiled postprocess block, elements of `0x7c`; its
/// animated properties at `+0x3c`, elements of `0x14`, the mapping
/// function a data reference at `+0x0c`.
pub const SHAD_ROOT: usize = 0x54;
pub const SHAD_POSTPROCESS: usize = 0x20;
pub const POSTPROCESS_SIZE: usize = 0x7c;
pub const POSTPROCESS_PROPERTIES: usize = 0x3c;
pub const PROPERTY_SIZE: usize = 0x14;
pub const PROPERTY_FUNCTION: usize = 0x0c;

/// The meter shader the health widget's shader is copied from.
pub const METER_SHADER: &str = "ui\\hud\\shaders\\ammo_meter";
/// The suffix of the converted HUD's name (`ui\hud\masterchief` becomes
/// `ui\hud\masterchief_h1_health`).
pub const NHDT_SUFFIX: &str = "_h1_health";
pub const SHADER_NAME: &str = "ui\\hud\\shaders\\h1_health_meter";

/// Whether the Halo 1 biped at `path` is the one the converters make the
/// player (`h1convert::convert::halo1` picks `cyborg_mp` or a biped named
/// `cyborg`; `h1campaign` takes `characters\cyborg\cyborg`).
pub fn is_player_biped(path: &str) -> bool {
    path.rsplit('\\')
        .next()
        .is_some_and(|last| last.contains("cyborg"))
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// The index of the shield meter among `widgets` (concatenated
/// [`BITMAP_WIDGET_SIZE`] elements): the first with special type 5.
pub fn shield_meter(widgets: &[u8]) -> Option<usize> {
    widgets
        .as_chunks::<BITMAP_WIDGET_SIZE>()
        .0
        .iter()
        .position(|w| u16_at(w, widget::SPECIAL) == SPECIAL_SHIELD_METER)
}

/// The health widget made from the shield meter's bytes: input 17, no
/// special type, no effects, raised [`RAISE`]. The shader reference is
/// left for the caller to point at the meter shader.
pub fn health_widget(shield: &[u8]) -> Vec<u8> {
    let mut w = shield[..BITMAP_WIDGET_SIZE].to_vec();
    w[widget::INPUTS..widget::INPUTS + 4].copy_from_slice(&[INPUT_BODY, 0, 0, 0]);
    w[widget::SPECIAL..widget::SPECIAL + 2].fill(0);
    w[widget::EFFECTS..widget::EFFECTS + 8].fill(0);
    for (at, raise) in widget::OFFSETS.iter().zip(RAISE) {
        let y = i16::from_le_bytes([w[at + 2], w[at + 3]]).saturating_sub(raise);
        w[at + 2..at + 4].copy_from_slice(&y.to_le_bytes());
    }
    w
}

/// The `nhdt`'s bitmap widget block with the health widget added after the
/// last widget, its shader `shader`. `None` when there is no shield meter.
pub fn widgets_with_health(widgets: &[u8], shader: TagId) -> Option<(Block, u32)> {
    let shield = shield_meter(widgets)?;
    let at = shield * BITMAP_WIDGET_SIZE;
    let mut bytes = widgets.to_vec();
    let end = bytes.len();
    bytes.extend(health_widget(&widgets[at..at + BITMAP_WIDGET_SIZE]));
    let mut block = Block::new(bytes);
    block.tag_ref((end + widget::SHADER) as u32, shader);
    Some((block, (end / BITMAP_WIDGET_SIZE + 1) as u32))
}

/// A mapping function's output range, when it is a plain range (byte 1's
/// high nibble 0: bytes `+4`/`+8` are the low and high `f32`).
pub fn function_range(f: &[u8]) -> Option<(f32, f32)> {
    if f.len() < 12 || f[1] & 0xf0 != 0 {
        return None;
    }
    let at = |o: usize| f32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    Some((at(4), at(8)))
}

/// `f` with its plain output range set to `range` (unchanged when it is
/// not a plain range).
pub fn with_range(f: &[u8], range: (f32, f32)) -> Vec<u8> {
    let mut out = f.to_vec();
    if function_range(f).is_some() {
        out[4..8].copy_from_slice(&range.0.to_le_bytes());
        out[8..12].copy_from_slice(&range.1.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::FixupKind;

    /// An invented widget: name, inputs, state flags, anchor, bitmap and
    /// shader references, sequences, offsets, special type.
    fn widget_bytes(name: u32, special: u16, offsets: [(i16, i16); 3]) -> Vec<u8> {
        let mut w = vec![0u8; BITMAP_WIDGET_SIZE];
        w[0..4].copy_from_slice(&name.to_le_bytes());
        for (i, b) in w[widget::STATE..widget::STATE_END].iter_mut().enumerate() {
            *b = 0x0b_u8.wrapping_mul(i as u8 + 1);
        }
        w[widget::BITMAP..widget::BITMAP + 8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        w[widget::SHADER..widget::SHADER + 8].copy_from_slice(&[9, 9, 9, 9, 8, 8, 8, 8]);
        w[0x30..0x34].copy_from_slice(&[0, 2, 2, 0]);
        for (at, (x, y)) in widget::OFFSETS.iter().zip(offsets) {
            w[*at..at + 2].copy_from_slice(&x.to_le_bytes());
            w[at + 2..at + 4].copy_from_slice(&y.to_le_bytes());
        }
        w[widget::EFFECTS..widget::EFFECTS + 8].copy_from_slice(&[1, 0, 0, 0, 0x10, 0, 0, 0x80]);
        w[widget::SPECIAL..widget::SPECIAL + 2].copy_from_slice(&special.to_le_bytes());
        w
    }

    fn hud() -> Vec<u8> {
        let mut b = widget_bytes(1, 0, [(0, 0); 3]);
        b.extend(widget_bytes(
            2,
            SPECIAL_SHIELD_METER,
            [(-1, -98), (-1, -76), (-1, -76)],
        ));
        b.extend(widget_bytes(3, 0, [(-1, -84); 3]));
        b
    }

    #[test]
    fn the_shield_meter_is_the_widget_with_special_type_5() {
        assert_eq!(shield_meter(&hud()), Some(1));
        assert_eq!(shield_meter(&widget_bytes(1, 0, [(0, 0); 3])), None);
    }

    #[test]
    fn the_health_widget_reads_the_body_input_and_has_the_shields_gate() {
        let h = hud();
        let shield = &h[BITMAP_WIDGET_SIZE..2 * BITMAP_WIDGET_SIZE];
        let w = health_widget(shield);
        // Input 17 is HUD state +0x28, the body (0x13ad48); 16 would be the
        // shield.
        assert_eq!(w[widget::INPUTS..widget::INPUTS + 4], [INPUT_BODY, 0, 0, 0]);
        assert_eq!(INPUT_BODY, INPUT_SHIELD + 1);
        // Not the shield's code path, no effects.
        assert_eq!(u16_at(&w, widget::SPECIAL), 0);
        assert!(
            w[widget::EFFECTS..widget::EFFECTS + 8]
                .iter()
                .all(|b| *b == 0)
        );
        // Everything 0x13ac87 tests, the anchor and flags, the bitmap and
        // the sequences are the shield meter's.
        assert_eq!(w[widget::STATE..0x20], shield[widget::STATE..0x20]);
        assert_eq!(
            w[widget::BITMAP..widget::BITMAP + 8],
            shield[widget::BITMAP..widget::BITMAP + 8]
        );
        assert_eq!(w[0x30..0x34], shield[0x30..0x34]);
        assert_eq!(w[0..4], shield[0..4]);
    }

    #[test]
    fn the_health_arc_sits_above_the_shield_arc_in_every_screen_mode() {
        let h = hud();
        let shield = &h[BITMAP_WIDGET_SIZE..2 * BITMAP_WIDGET_SIZE];
        let w = health_widget(shield);
        let point = |b: &[u8], at: usize| {
            (
                i16::from_le_bytes([b[at], b[at + 1]]),
                i16::from_le_bytes([b[at + 2], b[at + 3]]),
            )
        };
        for (at, raise) in widget::OFFSETS.iter().zip(RAISE) {
            let (sx, sy) = point(shield, *at);
            assert_eq!(point(&w, *at), (sx, sy - raise));
            assert!(raise > 0);
        }
    }

    #[test]
    fn the_widget_block_gains_one_widget_pointing_at_the_meter_shader() {
        let h = hud();
        let (block, count) = widgets_with_health(&h, TagId(0xfffd_0007)).unwrap();
        assert_eq!(count, 4);
        assert_eq!(block.bytes.len(), 4 * BITMAP_WIDGET_SIZE);
        assert_eq!(block.bytes[..h.len()], h[..]);
        assert_eq!(
            block.bytes[h.len()..],
            health_widget(&h[BITMAP_WIDGET_SIZE..2 * BITMAP_WIDGET_SIZE])[..]
        );
        assert_eq!(block.fixups.len(), 1);
        assert_eq!(block.fixups[0].offset as usize, h.len() + widget::SHADER);
        assert_eq!(block.fixups[0].kind, FixupKind::TagRef(TagId(0xfffd_0007)));
        assert!(widgets_with_health(&widget_bytes(1, 0, [(0, 0); 3]), TagId(1)).is_none());
    }

    #[test]
    fn the_meter_function_takes_the_shields_range() {
        let mut f = vec![0u8; 20];
        f[8..12].copy_from_slice(&1.0f32.to_le_bytes());
        assert_eq!(function_range(&f), Some((0.0, 1.0)));
        let g = with_range(&f, (0.2156, 0.992));
        assert_eq!(function_range(&g), Some((0.2156, 0.992)));
        assert_eq!(g[12..], f[12..]);
        // A colour function (byte 1 high nibble set) is left alone.
        let mut c = f.clone();
        c[1] = 0x20;
        assert_eq!(function_range(&c), None);
        assert_eq!(with_range(&c, (0.5, 0.6)), c);
    }

    #[test]
    fn only_the_cyborg_is_the_player_biped() {
        assert!(is_player_biped("characters\\cyborg\\cyborg"));
        assert!(is_player_biped("characters\\cyborg_mp\\cyborg_mp"));
        assert!(!is_player_biped("characters\\marine\\marine"));
        assert!(!is_player_biped("characters\\cyborg\\elite"));
    }
}
