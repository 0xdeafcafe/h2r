//! Where the engine puts a widget on screen, and how sure we are.
//!
//! Every rule carries a [`Proof`]: **E** read in the engine's code (the
//! retail `default.xbe`), **V** seen on the owner's disc or in a frame of
//! the game, **D** assumed here. The editor shows the list ([`RULES`]) so
//! nothing unproven passes for proven.
//!
//! Coordinates are HUD pixels, x right and y down, at 640 by 480
//! fullscreen.

use crate::layout::Mode;

/// How a rule is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof {
    /// Read in the engine's code.
    Engine,
    /// Seen on the disc or in a frame of the game.
    Seen,
    /// Assumed: not proven.
    Assumed,
}

impl Proof {
    pub fn tag(self) -> &'static str {
        match self {
            Proof::Engine => "E",
            Proof::Seen => "V",
            Proof::Assumed => "D",
        }
    }
}

/// One placement rule, for the editor's "placement rules" list.
pub struct Rule {
    pub what: &'static str,
    pub proof: Proof,
    pub cite: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule {
        what: "Anchor point = a corner of the view's HUD frame (camera +0x38: top, left, bottom, right): 0 top right, 1 top left, 2 bottom left, 3 bottom right, 4 the frame's centre",
        proof: Proof::Engine,
        cite: "0x1396c7 (frame at 0x4b9dd8..0x4b9dde)",
    },
    Rule {
        what: "Anchor 4 (centre): y follows the projected aim point when a player view is live; the preview uses the frame's centre",
        proof: Proof::Engine,
        cite: "0x13978a..0x1398bc",
    },
    Rule {
        what: "Anchors above 4 are placed by another path (not modelled: drawn at the frame's centre and flagged)",
        proof: Proof::Engine,
        cite: "0x1396f4",
    },
    Rule {
        what: "Bottom-left corner of the fullscreen 640x480 frame is (48, 444)",
        proof: Proof::Seen,
        cite: "a10 frame: motion tracker rim at x 47..129, y 360..442 (health_hud tests)",
    },
    Rule {
        what: "The frame is the screen inset 7.5% on every side: (48, 36)..(592, 444) at 640x480",
        proof: Proof::Assumed,
        cite: "symmetric with the seen corner; top and right not yet seen",
    },
    Rule {
        what: "Bitmap widget: point = anchor + offset of the screen mode; the sprite is placed so its registration (fractions of its size) lands on the point",
        proof: Proof::Engine,
        cite: "0x22aff1, 0x22b128..0x22b179",
    },
    Rule {
        what: "Sprite: the sequence of the screen mode; with sprites, the first sprite's bounds in its image; without, the sequence's first image whole; no sequences, image 0",
        proof: Proof::Engine,
        cite: "0x22ae21, 0x22ae94..0x22aeeb, 0x22aef0..0x22af3c",
    },
    Rule {
        what: "Sprites are drawn 1:1 in HUD pixels at 640x480 fullscreen",
        proof: Proof::Seen,
        cite: "a10 frame (motion tracker 83x83)",
    },
    Rule {
        what: "Some sequences draw their second image instead (0x22acb4); the preview always draws the first",
        proof: Proof::Assumed,
        cite: "condition not read",
    },
    Rule {
        what: "Text widget: point = anchor + offset; justification 0 starts at the point, 1 centres on it, 2 ends at it; the text's top is the point",
        proof: Proof::Engine,
        cite: "0x22aa16, 0x22ab17..0x22ac64",
    },
    Rule {
        what: "Text font: byte +0x30 of the mode, 0 engine font 6, 1 engine font 8 (font_table.txt line), else not drawn",
        proof: Proof::Engine,
        cite: "0x22a85b, 0x22a98e..0x22a9b6",
    },
    Rule {
        what: "Widget effects (+0x58, text +0x4c) move, scale and fade widgets at run time; the preview ignores them",
        proof: Proof::Engine,
        cite: "0x22bcaf (not modelled)",
    },
    Rule {
        what: "Shown or hidden by the state gate: shown when any show word meets the state, unless a hide word does",
        proof: Proof::Engine,
        cite: "0x13ac87",
    },
    Rule {
        what: "Meters fill where the texture's blue is below hud_input1 (ammo_meter, widget_legacy_meter); other shaders are drawn as plain textures",
        proof: Proof::Seen,
        cite: "docs/halo1/health.md; shader colours not modelled",
    },
];

/// The view's HUD frame (render camera `+0x38`, `0x4b9dd8`): i16 top,
/// left, bottom, right in screen pixels (E `0x1396c7` reads it in that
/// order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}

/// The screen the preview draws on.
pub const SCREEN: [f32; 2] = [640.0, 480.0];

impl Frame {
    /// Fullscreen at 640 by 480: the bottom-left corner seen (V), the rest
    /// assumed symmetric (D).
    pub const FULLSCREEN: Frame = Frame {
        top: 36,
        left: 48,
        bottom: 444,
        right: 592,
    };

    /// The frame for a screen mode, as the preview lays split screen out:
    /// the top player's half (half) or the top-left quarter (quarter), each
    /// inset 7.5% (D: split-screen frames are not read).
    pub fn for_mode(mode: Mode) -> Frame {
        let inset = |w: f32, h: f32| Frame {
            top: (h * 0.075).round() as i16,
            left: (w * 0.075).round() as i16,
            bottom: (h * 0.925).round() as i16,
            right: (w * 0.925).round() as i16,
        };
        match mode {
            Mode::Full => Frame::FULLSCREEN,
            Mode::Half => inset(640.0, 240.0),
            Mode::Quarter => inset(320.0, 240.0),
        }
    }

    /// The point anchor `a` names (E `0x1396c7`): `None` for an anchor
    /// the corner switch does not handle (above 4).
    pub fn anchor_point(&self, a: u16) -> Option<[f32; 2]> {
        let [t, l, b, r] = [self.top, self.left, self.bottom, self.right].map(f32::from);
        Some(match a {
            // 0x139908: x the right, y the top.
            0 => [r, t],
            // 0x1398f4: x the left, y the top.
            1 => [l, t],
            // 0x1398eb: x the left, y the bottom.
            2 => [l, b],
            // 0x1398e2: x the right, y the bottom.
            3 => [r, b],
            // 0x13978a: the midpoints (integer halves).
            4 => [
                ((self.left as i32 + self.right as i32) / 2) as f32,
                ((self.top as i32 + self.bottom as i32) / 2) as f32,
            ],
            _ => return None,
        })
    }

    pub fn rect(&self) -> Rect {
        Rect {
            x0: f32::from(self.left),
            y0: f32::from(self.top),
            x1: f32::from(self.right),
            y1: f32::from(self.bottom),
        }
    }
}

/// A rectangle in HUD pixels, ends exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }
    pub fn contains_point(&self, p: [f32; 2]) -> bool {
        p[0] >= self.x0 && p[0] < self.x1 && p[1] >= self.y0 && p[1] < self.y1
    }
    pub fn contains(&self, o: &Rect) -> bool {
        o.x0 >= self.x0 && o.y0 >= self.y0 && o.x1 <= self.x1 && o.y1 <= self.y1
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        };
        (!r.is_empty()).then_some(r)
    }
    /// Rounded to whole pixels, `[x0, y0, x1, y1]`.
    pub fn pixels(&self) -> [i32; 4] {
        [self.x0, self.y0, self.x1, self.y1].map(|v| v.round() as i32)
    }
}

/// A bitmap widget's sprite on screen (E `0x22b14c`..`0x22b234`): the
/// registration point `reg` (fractions of the sprite's `size`) lands on
/// `point`, so the sprite spans `point - reg * size` to
/// `point + (1 - reg) * size`.
pub fn bitmap_rect(point: [f32; 2], size: [f32; 2], reg: [f32; 2]) -> Rect {
    let x0 = point[0] - reg[0] * size[0];
    let y0 = point[1] - reg[1] * size[1];
    Rect {
        x0,
        y0,
        x1: x0 + size[0],
        y1: y0 + size[1],
    }
}

/// A text widget's text on screen (E `0x22ac0b`..`0x22ac64`): the top
/// at the point, justified across it.
pub fn text_rect(point: [f32; 2], size: [f32; 2], justification: u16) -> Rect {
    let x0 = match justification {
        1 => point[0] - (size[0] / 2.0).floor(),
        2 => point[0] - size[0],
        _ => point[0],
    };
    Rect {
        x0,
        y0: point[1],
        x1: x0 + size[0],
        y1: point[1] + size[1],
    }
}

/// The point a widget is placed from: its anchor's point plus its offset.
/// `None` when the anchor is not placed by the corner switch.
pub fn widget_point(frame: &Frame, anchor: u16, offset: (i16, i16)) -> Option<[f32; 2]> {
    let p = frame.anchor_point(anchor)?;
    Some([p[0] + f32::from(offset.0), p[1] + f32::from(offset.1)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_corners_follow_the_engines_switch() {
        let f = Frame::FULLSCREEN;
        assert_eq!(f.anchor_point(0), Some([592.0, 36.0]));
        assert_eq!(f.anchor_point(1), Some([48.0, 36.0]));
        assert_eq!(f.anchor_point(2), Some([48.0, 444.0]));
        assert_eq!(f.anchor_point(3), Some([592.0, 444.0]));
        assert_eq!(f.anchor_point(4), Some([320.0, 240.0]));
        assert_eq!(f.anchor_point(5), None);
    }

    /// Retail `ui\hud\masterchief` (docs/halo1/health.md, D read from
    /// shared.map): the motion tracker, anchor 2 at (-1, -84), 83 x 83,
    /// registered at its top left, sits at x 47..130, y 360..443, which a
    /// frame of a10 shows (V); the shield meter, (-1, -98), 128 x 32, at
    /// x 47..175, y 346..378. Both inside the screen and the tracker
    /// inside the frame.
    #[test]
    fn retail_masterchief_widgets_land_where_documented() {
        let f = Frame::FULLSCREEN;
        let sensor = bitmap_rect(widget_point(&f, 2, (-1, -84)).unwrap(), [83.0, 83.0], [0.0, 0.0]);
        assert_eq!(sensor.pixels(), [47, 360, 130, 443]);
        let shield = bitmap_rect(widget_point(&f, 2, (-1, -98)).unwrap(), [128.0, 32.0], [0.0, 0.0]);
        assert_eq!(shield.pixels(), [47, 346, 175, 378]);
        let screen = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: SCREEN[0],
            y1: SCREEN[1],
        };
        assert!(screen.contains(&sensor) && screen.contains(&shield));
        // The health strip the converter adds above it (-1, -108), 92 x 8.
        let strip = bitmap_rect(widget_point(&f, 2, (-1, -108)).unwrap(), [92.0, 8.0], [0.0, 0.0]);
        assert_eq!(strip.pixels(), [47, 336, 139, 344]);
        assert!(strip.intersect(&shield).is_none());
    }

    #[test]
    fn registration_puts_its_point_on_the_anchor() {
        let r = bitmap_rect([100.0, 50.0], [40.0, 20.0], [1.0, 1.0]);
        assert_eq!(r.pixels(), [60, 30, 100, 50]);
        let r = bitmap_rect([100.0, 50.0], [40.0, 20.0], [0.5, 0.5]);
        assert_eq!(r.pixels(), [80, 40, 120, 60]);
    }

    /// Retail `frag_count`: anchor 1 (top left), (65, -1), centred
    /// (docs/halo1/hud.md): its point is (113, 35); a 7-pixel-wide digit
    /// centres on it.
    #[test]
    fn text_justification() {
        let p = widget_point(&Frame::FULLSCREEN, 1, (65, -1)).unwrap();
        assert_eq!(p, [113.0, 35.0]);
        assert_eq!(text_rect(p, [7.0, 12.0], 1).pixels(), [110, 35, 117, 47]);
        assert_eq!(text_rect(p, [7.0, 12.0], 0).pixels(), [113, 35, 120, 47]);
        assert_eq!(text_rect(p, [7.0, 12.0], 2).pixels(), [106, 35, 113, 47]);
    }

    #[test]
    fn split_screen_frames_are_inside_their_screen() {
        for m in Mode::ALL {
            let f = Frame::for_mode(m);
            assert!(f.left > 0 && f.top > 0 && f.right < 640 && f.bottom < 480, "{m:?}");
        }
    }
}
