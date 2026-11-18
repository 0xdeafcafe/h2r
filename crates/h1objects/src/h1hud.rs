//! Halo 1's unit HUD (`unhi`) as a Halo 2 HUD (`nhdt`), written from
//! scratch: every byte of the `nhdt` and its widgets is set here, none is a
//! template's (docs/game/hud.md).
//!
//! Halo 2 draws an `nhdt`'s bitmap widgets one by one (`0x13a532`), each a
//! sprite of its bitmap through a HUD shader, placed by anchor, offset and
//! registration, and fed by up to four inputs (`0x13ad48`). Halo 1's unit
//! HUD is a fixed set of elements, each a sprite of a bitmap placed by an
//! anchor and offset, the meters filled by the shield and the body. So each
//! Halo 1 element becomes one widget:
//!
//! - the bitmap is Halo 1's own, converted unchanged (`h1bitmaps`): Halo
//!   1's HUD bitmaps are sprite sheets, and Halo 2's widget draws a
//!   sequence's sprite when it has one (E `0x22ae94`..`0x22aeeb`: sprite
//!   count `+0x34` > 0, the sprite's image and its bounds `+8..+0x18`);
//! - a meter draws through Halo 2's legacy meter shader ([`METER_SHADER`],
//!   template `widget_legacy_meter`: fills where the texture's blue is below
//!   `hud_input1`). Halo 1's meter sprites are A8Y8, a luminance ramp under
//!   an alpha mask (V: `hud_unit_meters`), so the blue the shader reads is
//!   Halo 1's ramp: the fill order is Halo 1's;
//! - the shield meter reads input 16 (the shield), the health meter input
//!   17 (the body; `0x13ad48` cases `0x10`, `0x11`), with no special type
//!   (5 is Halo 2's own shield meter code, `0x22aff1`);
//! - backgrounds draw through [`STATIC_SHADER`];
//! - every widget has the state flags of Halo 2's shield meter
//!   ([`STATE`]), so the per-widget gate `0x13ac87` answers as for the
//!   shield meter, and the whole `nhdt` is behind HUD_SHOWN (`0x13939b`):
//!   pause menu, cinematics, Blind and death hide it as they hide Halo 2's.

use h1map::{H1Map, TagId as H1TagId};
use map::TagId;
use map::build::Block;

use crate::health_hud::{BITMAP_WIDGET_SIZE, NHDT_BITMAP_WIDGETS, NHDT_ROOT, widget};
use crate::{Error, Result};

/// The legacy meter shader (`widget_legacy_meter`) Halo 2's own ammo
/// meters use.
pub const METER_SHADER: &str = "ui\\hud\\shaders\\ammo_meter";
/// The plain widget shader Halo 2's HUD backgrounds use
/// (`ui\hud\masterchief` grenade and motion tracker backgrounds).
pub const STATIC_SHADER: &str = "ui\\hud\\shaders\\widget_gradient_flash";

/// The suffix of a converted Halo 1 unit HUD's name.
pub const NHDT_SUFFIX: &str = "_h1_hud";

/// Widget state flags (`+0x08..+0x1c`, read by the gate `0x13ac87`): those
/// of Halo 2's shield meter (V: `ui\hud\masterchief` widget
/// `shield_meter`), shown whenever the unit's HUD is.
pub const STATE: [u8; 20] = [
    0x00, 0x10, 0, 0, 0, 0, 0, 0, 0x80, 0x05, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Halo 2 widget anchors (`+0x1c`, E `0x1396c7`: the safe area's corner
/// it names, centre for 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Anchor {
    TopRight = 0,
    TopLeft = 1,
    BottomLeft = 2,
    BottomRight = 3,
    Centre = 4,
}

impl Anchor {
    /// Halo 1's anchor enumeration (`unhi +0`): top left, top right,
    /// bottom left, bottom right, centre (E, Halo 1's XBE: `0x000e0330`
    /// lays the sprite out from the anchor by a jump table on it, rightwards
    /// for 0 and 2, leftwards for 1 and 3, downwards for 0 and 1, upwards
    /// for 2 and 3, centred for 4; `0x000e07a0` takes the corner bit 0 for
    /// right, bit 1 for bottom).
    pub fn from_halo1(v: u16) -> Option<Self> {
        Some(match v {
            0 => Anchor::TopLeft,
            1 => Anchor::TopRight,
            2 => Anchor::BottomLeft,
            3 => Anchor::BottomRight,
            4 => Anchor::Centre,
            _ => return None,
        })
    }
}

/// Halo 1 HUD element layout (V: `ui\hud\cyborg` and `cyborg_mp`, each
/// element's bitmap reference resolving at `+0x24`, every map).
pub mod h1el {
    /// Anchor offset, two `i16` (x, y).
    pub const OFFSET: usize = 0x00;
    /// Width and height scale, two `f32` (1.0 on the cyborg).
    pub const SCALE: usize = 0x04;
    /// The bitmap (16-byte Halo 1 reference).
    pub const BITMAP: usize = 0x24;
    /// A static element's colour (`u32` ARGB).
    pub const COLOUR: usize = 0x34;
    /// A static element's sequence (`i16`).
    pub const SEQUENCE: usize = 0x54;
    /// A meter's sequence (`i16`; the byte before is its minimum value).
    pub const METER_SEQUENCE: usize = 0x46;
}

/// Where the unit HUD's elements are in `unhi` (V: an element's offset
/// pair, scale and bitmap reference at these, the bitmaps
/// `hud_unit_backgrounds`, `hud_unit_meters` and
/// `hud_weapon_backgrounds`).
pub mod unhi {
    /// The anchor of everything but the motion sensor (`u16`).
    pub const ANCHOR: usize = 0x00;
    pub const SHIELD_BACKGROUND: usize = 0x8c;
    pub const SHIELD_METER: usize = 0xf4;
    pub const HEALTH_BACKGROUND: usize = 0x17c;
    pub const HEALTH_METER: usize = 0x1e4;
    pub const MOTION_BACKGROUND: usize = 0x26c;
    pub const MOTION_FOREGROUND: usize = 0x2d4;
    /// The root's size (V: every `unhi` is at least this long, the
    /// cyborg's with its blocks after it).
    pub const SIZE: usize = 0x56c;
}

/// What a Halo 1 element shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Static,
    /// A meter on Halo 2 input `n` (16 shield, 17 body).
    Meter(u8),
}

/// One Halo 1 HUD element.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: &'static str,
    pub role: Role,
    pub anchor: Anchor,
    pub offset: (i16, i16),
    pub scale: [f32; 2],
    pub bitmap: H1TagId,
    pub sequence: i16,
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The drawable elements of the `unhi` `b` (those with a bitmap), in draw
/// order: backgrounds before meters, the motion sensor last.
pub fn unit_elements(map: &H1Map, b: &[u8]) -> Result<Vec<Element>> {
    if b.len() < unhi::SIZE {
        return Err(Error::Truncated("unhi"));
    }
    let raw = u16::from_le_bytes([b[unhi::ANCHOR], b[unhi::ANCHOR + 1]]);
    let anchor = Anchor::from_halo1(raw)
        .ok_or_else(|| Error::Invalid(format!("unhi anchor {raw}")))?;
    // Halo 1 draws the motion sensor's elements with anchor 2, bottom left
    // (E `0x000dd5ab`: a stand-in header whose anchor word is 2 is passed
    // to the element draw `0x000e1830` in place of the `unhi`).
    let rows = [
        ("h1_shield_background", unhi::SHIELD_BACKGROUND, Role::Static, anchor),
        ("h1_health_background", unhi::HEALTH_BACKGROUND, Role::Static, anchor),
        ("h1_shield_meter", unhi::SHIELD_METER, Role::Meter(16), anchor),
        ("h1_health_meter", unhi::HEALTH_METER, Role::Meter(17), anchor),
        ("h1_motion_background", unhi::MOTION_BACKGROUND, Role::Static, Anchor::BottomLeft),
        ("h1_motion_foreground", unhi::MOTION_FOREGROUND, Role::Static, Anchor::BottomLeft),
    ];
    let mut out = Vec::new();
    for (name, at, role, anchor) in rows {
        let Some(bitmap) = crate::h1::reference(map, b, at + h1el::BITMAP) else {
            continue;
        };
        let seq_at = match role {
            Role::Static => h1el::SEQUENCE,
            Role::Meter(_) => h1el::METER_SEQUENCE,
        };
        out.push(Element {
            name,
            role,
            anchor,
            offset: to_halo2_offset(
                anchor,
                (i16_at(b, at + h1el::OFFSET), i16_at(b, at + h1el::OFFSET + 2)),
            ),
            scale: [f32_at(b, at + h1el::SCALE), f32_at(b, at + h1el::SCALE + 4)],
            bitmap,
            sequence: i16_at(b, at + seq_at),
        });
    }
    Ok(out)
}

/// Halo 1's offset as Halo 2's. Halo 1 measures it inward from the
/// anchor's corner (E `0x000e07a0`: x times -1 when anchor bit 0, the right
/// edge, is set; y times -1 when bit 1, the bottom, is set; centre as
/// is); Halo 2 adds it in screen directions (the health strip's -108 is
/// up from a bottom anchor, docs/game/health.md).
pub fn to_halo2_offset(anchor: Anchor, (x, y): (i16, i16)) -> (i16, i16) {
    let right = matches!(anchor, Anchor::TopRight | Anchor::BottomRight);
    let bottom = matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight);
    (
        if right { x.saturating_neg() } else { x },
        if bottom { y.saturating_neg() } else { y },
    )
}

/// A Halo 2 bitmap widget, every field of its `0x64` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// `+0x00` the name (string ID), for the debug text only.
    pub name: String,
    /// `+0x04` the four inputs (`0x13ad48`).
    pub inputs: [u8; 4],
    /// `+0x08..+0x1c` state flags (`0x13ac87`).
    pub state: [u8; 20],
    /// `+0x1c` anchor, `+0x1e` flags (bits 0 and 1 mirror, `0x22b29c`).
    pub anchor: Anchor,
    pub flags: u16,
    /// `+0x20` bitmap, `+0x28` shader.
    pub bitmap: TagId,
    pub shader: TagId,
    /// `+0x30` sequence per screen mode (fullscreen, half, quarter), `+0x33`
    /// zero.
    pub sequence: i8,
    /// `+0x34..+0x40` offsets per screen mode.
    pub offset: (i16, i16),
    /// `+0x40..+0x58` registration per screen mode (fractions of the
    /// sprite's width and height).
    pub registration: [f32; 2],
    /// `+0x58` effects: none. `+0x60` special type, `+0x62` zero.
    pub special: u16,
}

impl Widget {
    /// The widget's bytes, and the offsets of its bitmap and shader
    /// references.
    pub fn bytes(&self) -> Vec<u8> {
        let mut w = vec![0u8; BITMAP_WIDGET_SIZE];
        w[widget::INPUTS..widget::INPUTS + 4].copy_from_slice(&self.inputs);
        w[widget::STATE..widget::STATE_END].copy_from_slice(&self.state);
        w[widget::ANCHOR..widget::ANCHOR + 2].copy_from_slice(&(self.anchor as u16).to_le_bytes());
        w[0x1e..0x20].copy_from_slice(&self.flags.to_le_bytes());
        w[widget::SEQUENCES..widget::SEQUENCES + 3].fill(self.sequence as u8);
        for at in widget::OFFSETS {
            w[at..at + 2].copy_from_slice(&self.offset.0.to_le_bytes());
            w[at + 2..at + 4].copy_from_slice(&self.offset.1.to_le_bytes());
        }
        for m in 0..3 {
            let at = widget::REGISTRATION + 8 * m;
            w[at..at + 4].copy_from_slice(&self.registration[0].to_le_bytes());
            w[at + 4..at + 8].copy_from_slice(&self.registration[1].to_le_bytes());
        }
        w[widget::SPECIAL..widget::SPECIAL + 2].copy_from_slice(&self.special.to_le_bytes());
        w
    }
}

/// What of `bm`'s sequence `seq` a widget draws: its bounds (left, right,
/// top, bottom, fractions of the image) and registration point, image
/// fractions. A sequence with sprites draws its first sprite; one without
/// draws its first image whole (E `0x22aef0`..`0x22af3c`), registered at
/// the image's registration point (pixels).
pub fn sprite(bm: &h1map::bitmap::Bitmap, seq: i16) -> Option<([f32; 4], [f32; 2])> {
    let s = bm.sequences.get(usize::try_from(seq).ok()?)?;
    if let Some(p) = s.sprites.first() {
        return Some((p.bounds, p.registration));
    }
    if s.image_count <= 0 {
        return None;
    }
    let img = bm.images.get(usize::try_from(s.first_image).ok()?)?;
    let (w, h) = (f32::from(img.width.max(1)), f32::from(img.height.max(1)));
    Some((
        [0.0, 1.0, 0.0, 1.0],
        [f32::from(img.registration[0]) / w, f32::from(img.registration[1]) / h],
    ))
}

/// The registration a widget gets: the sprite's corner on its anchor's
/// side (its right edge on a right anchor, its bottom on a bottom one, its
/// middle on the centre), so anchor + offset is that corner, as Halo 1
/// lays it out (E `0x000e0330`: the sprite's rectangle from the placed
/// point is `[0, w]` or `[-w, 0]` across and `[0, h]` or `[-h, 0]` down by
/// the anchor, centred for 4). Halo 1 never reads the sprite's own
/// registration point there.
pub fn registration(anchor: Anchor) -> [f32; 2] {
    match anchor {
        Anchor::TopLeft => [0.0, 0.0],
        Anchor::TopRight => [1.0, 0.0],
        Anchor::BottomLeft => [0.0, 1.0],
        Anchor::BottomRight => [1.0, 1.0],
        Anchor::Centre => [0.5, 0.5],
    }
}

/// The widget for Halo 1 element `e`, its bitmap converted as `bitmap`,
/// drawn with `meter` or `plain` by its role.
pub fn widget_for(
    e: &Element,
    bitmap: TagId,
    registration: [f32; 2],
    meter: TagId,
    plain: TagId,
) -> Widget {
    let (inputs, shader) = match e.role {
        Role::Static => ([0; 4], plain),
        Role::Meter(input) => ([input, 0, 0, 0], meter),
    };
    Widget {
        name: e.name.to_owned(),
        inputs,
        state: STATE,
        anchor: e.anchor,
        flags: 0,
        bitmap,
        shader,
        sequence: e.sequence.clamp(0, i8::MAX as i16) as i8,
        offset: e.offset,
        registration,
        special: 0,
    }
}

/// The `nhdt` root drawing `widgets` and nothing else: the root's
/// reference null with its group word as on every retail `nhdt`, no text
/// widgets, no effects.
pub fn nhdt(widgets: &[Widget]) -> Block {
    let mut root = Block::new(vec![0; NHDT_ROOT]);
    root.bytes[0..4].copy_from_slice(&u32::from_be_bytes(*b"nhdt").to_le_bytes());
    root.bytes[4..8].fill(0xff);
    let mut block = Block::new(Vec::with_capacity(widgets.len() * BITMAP_WIDGET_SIZE));
    for w in widgets {
        let at = block.bytes.len() as u32;
        block.bytes.extend(w.bytes());
        block.string_id(at, &w.name);
        block.tag_ref(at + widget::BITMAP as u32, w.bitmap);
        block.tag_ref(at + widget::SHADER as u32, w.shader);
    }
    root.block(NHDT_BITMAP_WIDGETS as u32, widgets.len() as u32, block);
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::FixupKind;

    #[test]
    fn halo1_offsets_point_inward_from_the_corner() {
        assert_eq!(to_halo2_offset(Anchor::TopRight, (7, 3)), (-7, 3));
        assert_eq!(to_halo2_offset(Anchor::BottomLeft, (7, 3)), (7, -3));
        assert_eq!(to_halo2_offset(Anchor::BottomRight, (7, 3)), (-7, -3));
        assert_eq!(to_halo2_offset(Anchor::TopLeft, (7, 3)), (7, 3));
        assert_eq!(to_halo2_offset(Anchor::Centre, (7, 3)), (7, 3));
    }

    #[test]
    fn halo1_anchors_map_to_the_same_corners() {
        assert_eq!(Anchor::from_halo1(1), Some(Anchor::TopRight));
        assert_eq!(Anchor::from_halo1(0), Some(Anchor::TopLeft));
        assert_eq!(Anchor::from_halo1(2), Some(Anchor::BottomLeft));
        assert_eq!(Anchor::from_halo1(4), Some(Anchor::Centre));
        assert_eq!(Anchor::from_halo1(5), None);
    }

    #[test]
    fn registration_is_the_corner_on_the_anchors_side() {
        assert_eq!(registration(Anchor::TopRight), [1.0, 0.0]);
        assert_eq!(registration(Anchor::BottomLeft), [0.0, 1.0]);
        assert_eq!(registration(Anchor::Centre), [0.5, 0.5]);
    }

    fn element(role: Role) -> Element {
        Element {
            name: "h1_shield_meter",
            role,
            anchor: Anchor::TopRight,
            offset: (-7, 1),
            scale: [1.0, 1.0],
            bitmap: H1TagId(0),
            sequence: 3,
        }
    }

    #[test]
    fn a_meter_reads_its_input_through_the_meter_shader() {
        let w = widget_for(&element(Role::Meter(17)), TagId(1), [1.0, 0.0], TagId(2), TagId(3));
        let b = w.bytes();
        assert_eq!(b.len(), BITMAP_WIDGET_SIZE);
        assert_eq!(b[widget::INPUTS..widget::INPUTS + 4], [17, 0, 0, 0]);
        assert_eq!(w.shader, TagId(2));
        // Not Halo 2's shield meter code, no effects.
        assert_eq!(b[widget::SPECIAL..widget::SPECIAL + 2], [0, 0]);
        assert!(b[widget::EFFECTS..widget::EFFECTS + 8].iter().all(|x| *x == 0));
        assert_eq!(b[widget::STATE..widget::STATE_END], STATE);
        assert_eq!(b[widget::ANCHOR..widget::ANCHOR + 2], [0, 0]);
        assert_eq!(b[widget::SEQUENCES..widget::SEQUENCES + 4], [3, 3, 3, 0]);
        for at in widget::OFFSETS {
            assert_eq!(crate::health_hud::offset(&b, at), (-7, 1));
        }
        assert_eq!(f32_at(&b, widget::REGISTRATION), 1.0);
        assert_eq!(f32_at(&b, widget::REGISTRATION + 0x14), 0.0);
        let s = widget_for(&element(Role::Static), TagId(1), [0.0; 2], TagId(2), TagId(3));
        assert_eq!(s.inputs, [0; 4]);
        assert_eq!(s.shader, TagId(3));
    }

    #[test]
    fn the_nhdt_holds_only_the_widgets_written() {
        let ws: Vec<_> = [Role::Static, Role::Meter(16)]
            .iter()
            .map(|r| widget_for(&element(*r), TagId(1), [0.0; 2], TagId(2), TagId(3)))
            .collect();
        let root = nhdt(&ws);
        assert_eq!(root.bytes.len(), NHDT_ROOT);
        assert_eq!(&root.bytes[0..8], b"tdhn\xff\xff\xff\xff");
        // Bitmap widgets only: the text and effects blocks stay empty.
        assert!(root.bytes[0x10..NHDT_ROOT].iter().all(|b| *b == 0));
        let (_, child) = root
            .fixups
            .iter()
            .find_map(|f| match &f.kind {
                FixupKind::Block { child, .. } => Some((f.offset, child)),
                _ => None,
            })
            .expect("widget block");
        assert_eq!(child.bytes.len(), 2 * BITMAP_WIDGET_SIZE);
        let refs = child
            .fixups
            .iter()
            .filter(|f| matches!(f.kind, FixupKind::TagRef(_)))
            .count();
        assert_eq!(refs, 4);
    }
}
