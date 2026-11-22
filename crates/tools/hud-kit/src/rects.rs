//! The retail rectangles a kit element is placed beside, and the ones it
//! must stay out of (docs/game/hud-kit.md, "Anchors, the safe area and
//! scaling", "Anchoring next to retail art"). HUD pixels at 640 by 480
//! fullscreen, ends exclusive ([`Rect`]).
//!
//! The disc test (`tests/disc.rs`) checks the art rectangles against
//! `hud_edit`'s placement of the retail widgets.

pub use hud_edit::place::{Frame, Rect, SCREEN};

use crate::model::{Anchor, Place};

const fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// The fullscreen frame's bottom-left corner, anchor 2 (V: a frame of a10;
/// `hud_edit::place::Frame::FULLSCREEN`).
pub const BOTTOM_LEFT: [i16; 2] = [48, 444];

/// The motion tracker's 83 by 83 rim (V, docs/halo1/health.md).
pub const TRACKER: Rect = r(47.0, 360.0, 130.0, 443.0);
/// What shows of the shield meter: columns 0 to 82 and rows 0 to 24 of
/// its 128 by 32 bitmap, placed at (47, 346) (V, docs/halo1/health.md).
pub const SHIELD_BAND: Rect = r(47.0, 346.0, 130.0, 371.0);
/// The health strip the Halo 1 converter adds on converted maps: 92 by 8
/// at (-1, -108) from the bottom left (Code `h1objects::health_hud`).
pub const H1_HEALTH: Rect = r(47.0, 336.0, 139.0, 344.0);

/// Font 6's line height at 640 by 480 (V: `h2 font`, `conduit-13`).
pub const MESSAGE_LINE: f32 = 21.0;
/// The message routine's start point fullscreen: anchor 1 (top left)
/// then 60 pixels down (E `0x0024cfda`..`0x0024d002`; S the frame's top
/// is 36).
pub const PROMPT_START: [f32; 2] = [48.0, 96.0];
/// Pixels below the frame's top the message start is, fullscreen and
/// split screen (E `0x0024cfda`..`0x0024d002`).
pub const PROMPT_DOWN: [f32; 2] = [60.0, 35.0];
/// The first message line's box: from the start point to the frame's
/// right edge, five lines high (E `0x0024d244`..`0x0024d277`).
pub const PROMPT_LINE: Rect = r(48.0, 96.0, 592.0, 96.0 + 5.0 * MESSAGE_LINE);
/// The training box's margin around its text, each side and above and
/// below (E `0x001e72e9`..`0x001e7373`).
pub const TRAINING_PAD: [f32; 2] = [5.0, 1.0];
/// The band the training box takes, for a box of two lines as wide as the
/// frame (its frame art's outer edge is not decoded: S, decoding 3).
pub const TRAINING_BOX: Rect = r(
    48.0,
    240.0 - MESSAGE_LINE - TRAINING_PAD[1],
    592.0,
    240.0 + MESSAGE_LINE + TRAINING_PAD[1],
);
/// The reticle's area about the screen's centre (S: the widest retail
/// reticle is not measured).
pub const RETICLE: Rect = r(288.0, 208.0, 352.0, 272.0);

/// The rectangles no widget element may cover (the `retail` check).
pub const RESERVED: [(&str, Rect); 4] = [
    ("the prompt line", PROMPT_LINE),
    ("the training box", TRAINING_BOX),
    ("the Halo 1 health strip", H1_HEALTH),
    ("the reticle", RETICLE),
];

/// Pixels a kit element may sit outside the HUD frame: retail art does
/// too (the tracker and the shield meter start at x 47, the frame at 48).
pub const FRAME_SLACK: f32 = 1.0;

/// The screen as a rectangle.
pub const fn screen() -> Rect {
    r(0.0, 0.0, SCREEN[0], SCREEN[1])
}

/// A place whose top-left corner is `height` plus `gap` pixels above
/// `r`'s top and in line with its left edge, from the bottom-left anchor,
/// fullscreen at 640 by 480: "above the motion tracker" written once.
pub fn above(r: Rect, gap: i16, height: i16) -> Place {
    let [x0, y0, ..] = r.pixels();
    let [ax, ay] = BOTTOM_LEFT;
    Place::new(
        Anchor::BottomLeft,
        (x0 as i16 - ax, y0 as i16 - gap - height - ay),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_band_and_strip_sit_above_the_tracker_and_apart() {
        const { assert!(SHIELD_BAND.y0 < TRACKER.y0 && SHIELD_BAND.y1 > TRACKER.y0) };
        const { assert!(H1_HEALTH.y1 <= SHIELD_BAND.y0) };
        assert!(H1_HEALTH.intersect(&SHIELD_BAND).is_none());
        let f = Frame::FULLSCREEN;
        assert_eq!([f.left, f.bottom], BOTTOM_LEFT);
        assert_eq!(
            PROMPT_START,
            [f32::from(f.left), f32::from(f.top) + PROMPT_DOWN[0]]
        );
        assert_eq!(TRAINING_BOX.pixels(), [48, 218, 592, 262]);
    }

    /// `above` puts a strip's top-left corner its height and the
    /// gap above the rectangle's top, in line with its left edge: the
    /// health strip, 8 high, 2 above the shield band, is the converter's
    /// (-1, -108).
    #[test]
    fn above_is_the_converters_health_strip() {
        let p = above(SHIELD_BAND, 2, 8);
        assert_eq!(p.full, (-1, -108));
        let pt =
            hud_edit::place::widget_point(&Frame::FULLSCREEN, p.anchor as u16, p.full).unwrap();
        assert_eq!(pt, [H1_HEALTH.x0, H1_HEALTH.y0]);
    }
}
