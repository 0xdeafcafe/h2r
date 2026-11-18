//! Halo 1's health meter in the converted player's Halo 2 HUD
//! (docs/halo1/health.md, "The health meter in the HUD").
//!
//! A converted Halo 1 map runs Halo 1's health model (`boot::health`), so
//! its player needs to see the body's health. Halo 2's HUD already reads it:
//! a bitmap widget's input 17 is the unit's body vitality (`0x13ad48`,
//! case `0x11`: HUD state `+0x28`, which `0x13992a` fills from the body
//! `+0xec`). So the meter is a widget in the player's `nhdt`, built here
//! from the shield meter's widget (the one with special type 5):
//!
//! - its state flags, anchor and flags stay the shield meter's, so whatever
//!   hides the shield meter hides it (the whole HUD gate `0x13939b`, which
//!   Blind, cinematics and `show_hud` act on, and the widget's own state
//!   gate `0x13ac87`), and it moves with split screen like the shield meter;
//! - input 0 is 17 (body) and the special type 0 (type 5 is the shield
//!   code path in `0x22aff1`, which reads the shield, not the input);
//! - it draws Halo 2's own thin meter strip: the bitmap and shader the
//!   energy sword's battery meter uses ([`STRIP_BITMAP`], [`STRIP_SHADER`];
//!   `ui\hud\plasma_sword` widget 3, input 56), a 92 x 8 bar that the
//!   `widget_legacy_meter` template fills from the left as `hud_input1`
//!   rises from 0 to 1, which is what the body vitality is;
//! - it sits [`GAP`] HUD pixels above the shield meter's bitmap, its left
//!   edge on the shield meter's ([`offsets`], [`Rect`]).
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

/// Bitmap widget fields (E: `0x13a532`, `0x13ac87`, `0x13ad48`, `0x22ae21`,
/// `0x22aff1`).
pub mod widget {
    /// The four input bytes, turned into floats by `0x13ad48`.
    pub const INPUTS: usize = 0x04;
    /// State flags tested by `0x13ac87` (`+0x08`..`+0x1b`).
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    /// Anchor (`u16`, `0x13a690`, `0x1396c7`) and flags (`u16`, `0x22b29c`:
    /// bits 0 and 1 mirror the texture).
    pub const ANCHOR: usize = 0x1c;
    pub const BITMAP: usize = 0x20;
    pub const SHADER: usize = 0x28;
    /// The bitmap sequence (`i8`) for fullscreen, half and quarter screen
    /// (`0x22ae7f`, `0x22ae79`, `0x22ae73`).
    pub const SEQUENCES: usize = 0x30;
    /// Offsets (two `i16`, x then y) for fullscreen, half and quarter screen
    /// (`0x22b128`, `0x22b0da`, `0x22b089`).
    pub const OFFSETS: [usize; 3] = [0x34, 0x38, 0x3c];
    /// Registration point (two `f32`, a fraction of the bitmap's width and
    /// height) per screen mode (`0x22b156`, `0x22b108`, `0x22b0b7`):
    /// `0x22aff1` puts that point of the bitmap on anchor + offset.
    pub const REGISTRATION: usize = 0x40;
    pub const REGISTRATION_END: usize = 0x58;
    /// The widget's effect block (count, address).
    pub const EFFECTS: usize = 0x58;
    /// Special type (`u16`, `0x22b2d0`): 5 is the shield meter.
    pub const SPECIAL: usize = 0x60;
}

/// `bitm +0x44`: the images (count, address), `0x74` each, the width and
/// height `i16` at `+4` and `+6` (`0x22aecd`, `0x22aed3`).
pub const BITM_IMAGES: usize = 0x44;
pub const BITM_IMAGE_SIZE: usize = 0x74;

/// `0x13ad48`: input 16 is the unit's shield (HUD state `+0x2c`), 17 its
/// body (`+0x28`).
pub const INPUT_SHIELD: u8 = 16;
pub const INPUT_BODY: u8 = 17;
/// The special type `0x22aff1` draws as the shield meter.
pub const SPECIAL_SHIELD_METER: u16 = 5;

/// The strip the health widget draws, and its shader: the energy sword's
/// battery meter (`ui\hud\plasma_sword`, widget `battery_meter`).
pub const STRIP_BITMAP: &str = "ui\\hud\\bitmaps\\new_hud\\meters\\disintegrator_meter";
pub const STRIP_SHADER: &str = "ui\\hud\\shaders\\ammo_meter";
/// [`STRIP_BITMAP`]'s sequence 0 (images 0 and 1, `0x22acb4` picks one):
/// 92 x 8, opaque but for a chamfered bottom-left corner, its blue rising
/// from 0 at column 13 to 1 at the right, which the meter fills below
/// `hud_input1`.
pub const STRIP_SIZE: (i16, i16) = (92, 8);
/// The shield meter's bitmap per screen mode (`shield_meter` sequences 0
/// and 2, which `ui\hud\masterchief` draws in fullscreen and split screen):
/// 128 x 32 and 64 x 32. What shows of it starts at its top row.
pub const SHIELD_SIZE: [(i16, i16); 3] = [(128, 32), (64, 32), (64, 32)];
/// HUD pixels between the strip's bottom and the shield meter's top.
pub const GAP: i16 = 2;

/// The suffix of the converted HUD's name (`ui\hud\masterchief` becomes
/// `ui\hud\masterchief_h1_health`).
pub const NHDT_SUFFIX: &str = "_h1_health";

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

/// The (x, y) offset at `at` in widget `w`.
pub fn offset(w: &[u8], at: usize) -> (i16, i16) {
    (
        i16::from_le_bytes([w[at], w[at + 1]]),
        i16::from_le_bytes([w[at + 2], w[at + 3]]),
    )
}

/// A widget's bitmap on screen, in HUD pixels from its anchor point, with
/// the registration point at the top left (`0x22aff1`: corner = anchor +
/// offset + (u - registration) * width, the same down): `x0..x1`, `y0..y1`,
/// ends exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: i16,
    pub y0: i16,
    pub x1: i16,
    pub y1: i16,
}

impl Rect {
    pub fn at(offset: (i16, i16), size: (i16, i16)) -> Self {
        Rect {
            x0: offset.0,
            y0: offset.1,
            x1: offset.0 + size.0,
            y1: offset.1 + size.1,
        }
    }

    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
}

/// The health strip's offsets for the shield meter's: the same left edge,
/// its bottom [`GAP`] pixels above the shield meter's top.
pub fn offsets(shield: [(i16, i16); 3]) -> [(i16, i16); 3] {
    shield.map(|(x, y)| (x, y - GAP - STRIP_SIZE.1))
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
/// special type, no effects, sequence 0 of its bitmap in every screen
/// mode, registered at the top left, placed by [`offsets`]. Its bitmap and
/// shader references are left for the caller to point at [`STRIP_BITMAP`]
/// and [`STRIP_SHADER`].
pub fn health_widget(shield: &[u8]) -> Vec<u8> {
    let mut w = shield[..BITMAP_WIDGET_SIZE].to_vec();
    w[widget::INPUTS..widget::INPUTS + 4].copy_from_slice(&[INPUT_BODY, 0, 0, 0]);
    w[widget::SEQUENCES..widget::SEQUENCES + 3].fill(0);
    w[widget::REGISTRATION..widget::REGISTRATION_END].fill(0);
    w[widget::EFFECTS..widget::EFFECTS + 8].fill(0);
    w[widget::SPECIAL..widget::SPECIAL + 2].fill(0);
    let placed = offsets(widget::OFFSETS.map(|at| offset(shield, at)));
    for (at, (x, y)) in widget::OFFSETS.iter().zip(placed) {
        w[*at..at + 2].copy_from_slice(&x.to_le_bytes());
        w[at + 2..at + 4].copy_from_slice(&y.to_le_bytes());
    }
    w
}

/// The `nhdt`'s bitmap widget block with the health widget added after the
/// last widget, drawing `bitmap` with `shader`. `None` when there is no
/// shield meter.
pub fn widgets_with_health(widgets: &[u8], bitmap: TagId, shader: TagId) -> Option<(Block, u32)> {
    let shield = shield_meter(widgets)?;
    let at = shield * BITMAP_WIDGET_SIZE;
    let mut bytes = widgets.to_vec();
    let end = bytes.len();
    bytes.extend(health_widget(&widgets[at..at + BITMAP_WIDGET_SIZE]));
    let mut block = Block::new(bytes);
    block.tag_ref((end + widget::BITMAP) as u32, bitmap);
    block.tag_ref((end + widget::SHADER) as u32, shader);
    Some((block, (end / BITMAP_WIDGET_SIZE + 1) as u32))
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
        b.extend(widget_bytes(3, 0, [(-1, -84), (-1, -63), (-1, -63)]));
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
        // Not the shield's code path, no effects, the strip's one sequence,
        // registered at the top left.
        assert_eq!(u16_at(&w, widget::SPECIAL), 0);
        assert!(
            w[widget::EFFECTS..widget::EFFECTS + 8]
                .iter()
                .all(|b| *b == 0)
        );
        assert_eq!(w[widget::SEQUENCES..widget::SEQUENCES + 3], [0, 0, 0]);
        assert!(
            w[widget::REGISTRATION..widget::REGISTRATION_END]
                .iter()
                .all(|b| *b == 0)
        );
        // Everything 0x13ac87 tests, and the anchor and flags, are the
        // shield meter's.
        assert_eq!(w[widget::STATE..0x20], shield[widget::STATE..0x20]);
        assert_eq!(w[0..4], shield[0..4]);
    }

    /// `ui\hud\masterchief`'s shield meter offsets (D: `nhdt` widget 3)
    /// and its motion sensor's (widget 5, an 83 x 83 bitmap in fullscreen,
    /// 64 x 64 split).
    const SHIELD: [(i16, i16); 3] = [(-1, -98), (-1, -76), (-1, -76)];
    const SENSOR: [((i16, i16), (i16, i16)); 3] = [
        ((-1, -84), (83, 83)),
        ((-1, -63), (64, 64)),
        ((-1, -63), (64, 64)),
    ];

    #[test]
    fn the_strip_sits_two_pixels_above_the_shield_meter_in_every_screen_mode() {
        let h = hud();
        let w = health_widget(&h[BITMAP_WIDGET_SIZE..2 * BITMAP_WIDGET_SIZE]);
        let placed = widget::OFFSETS.map(|at| offset(&w, at));
        assert_eq!(placed, offsets(SHIELD));
        assert_eq!(placed[0], (-1, -108));
        for mode in 0..3 {
            let strip = Rect::at(placed[mode], STRIP_SIZE);
            let shield = Rect::at(SHIELD[mode], SHIELD_SIZE[mode]);
            let (at, size) = SENSOR[mode];
            let sensor = Rect::at(at, size);
            assert!(
                !strip.overlaps(&shield),
                "mode {mode}: {strip:?} {shield:?}"
            );
            assert!(
                !strip.overlaps(&sensor),
                "mode {mode}: {strip:?} {sensor:?}"
            );
            assert_eq!(shield.y0 - strip.y1, GAP, "mode {mode}");
            assert_eq!(strip.x0, shield.x0, "mode {mode}");
        }
        // Fullscreen at 640 x 480, the anchor (bottom left, 7.5% in) at
        // (48, 444): the frame of a10 at converter v33 has the motion
        // sensor's rim at x 47..129, y 360..442, which is SENSOR[0] there.
        let screen = |r: Rect| (r.x0 + 48, r.y0 + 444, r.x1 + 48, r.y1 + 444);
        assert_eq!(
            screen(Rect::at(SENSOR[0].0, SENSOR[0].1)),
            (47, 360, 130, 443)
        );
        assert_eq!(
            screen(Rect::at(SHIELD[0], SHIELD_SIZE[0])),
            (47, 346, 175, 378)
        );
        assert_eq!(screen(Rect::at(placed[0], STRIP_SIZE)), (47, 336, 139, 344));
    }

    #[test]
    fn the_widget_block_gains_one_widget_pointing_at_the_strip() {
        let h = hud();
        let (block, count) =
            widgets_with_health(&h, TagId(0xfffd_0006), TagId(0xfffd_0007)).unwrap();
        assert_eq!(count, 4);
        assert_eq!(block.bytes.len(), 4 * BITMAP_WIDGET_SIZE);
        assert_eq!(block.bytes[..h.len()], h[..]);
        assert_eq!(
            block.bytes[h.len()..],
            health_widget(&h[BITMAP_WIDGET_SIZE..2 * BITMAP_WIDGET_SIZE])[..]
        );
        let refs: Vec<_> = block
            .fixups
            .iter()
            .map(|f| (f.offset as usize, f.kind.clone()))
            .collect();
        assert_eq!(
            refs,
            [
                (
                    h.len() + widget::BITMAP,
                    FixupKind::TagRef(TagId(0xfffd_0006))
                ),
                (
                    h.len() + widget::SHADER,
                    FixupKind::TagRef(TagId(0xfffd_0007))
                ),
            ]
        );
        assert!(
            widgets_with_health(&widget_bytes(1, 0, [(0, 0); 3]), TagId(1), TagId(2)).is_none()
        );
    }

    #[test]
    fn only_the_cyborg_is_the_player_biped() {
        assert!(is_player_biped("characters\\cyborg\\cyborg"));
        assert!(is_player_biped("characters\\cyborg_mp\\cyborg_mp"));
        assert!(!is_player_biped("characters\\marine\\marine"));
        assert!(!is_player_biped("characters\\cyborg\\elite"));
    }
}
