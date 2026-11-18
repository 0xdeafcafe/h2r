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
    pub const HUD_BACKGROUND: usize = 0x24;
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
    /// Halo 1's colour (`+0x34`, ARGB).
    pub colour: u32,
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The drawable elements of the `unhi` `b` (those with a bitmap), in draw
/// order (E `0x000dcf8f`..`0x000dd63b`: the HUD background, each meter
/// before its panel, the motion sensor last).
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
        ("h1_hud_background", unhi::HUD_BACKGROUND, Role::Static, anchor),
        ("h1_shield_meter", unhi::SHIELD_METER, Role::Meter(16), anchor),
        ("h1_shield_background", unhi::SHIELD_BACKGROUND, Role::Static, anchor),
        ("h1_health_meter", unhi::HEALTH_METER, Role::Meter(17), anchor),
        ("h1_health_background", unhi::HEALTH_BACKGROUND, Role::Static, anchor),
        ("h1_motion_background", unhi::MOTION_BACKGROUND, Role::Static, Anchor::BottomLeft),
        ("h1_motion_foreground", unhi::MOTION_FOREGROUND, Role::Static, Anchor::BottomLeft),
    ];
    Ok(rows
        .into_iter()
        .filter_map(|(name, at, role, anchor)| element_at(map, b, at, name, role, anchor))
        .collect())
}

/// The element at `at` of the HUD tag `b`, when it has a bitmap.
fn element_at(
    map: &H1Map,
    b: &[u8],
    at: usize,
    name: &'static str,
    role: Role,
    anchor: Anchor,
) -> Option<Element> {
    let bitmap = crate::h1::reference(map, b, at + h1el::BITMAP)?;
    let seq_at = match role {
        Role::Static => h1el::SEQUENCE,
        Role::Meter(_) => h1el::METER_SEQUENCE,
    };
    Some(Element {
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
        colour: u32::from_le_bytes(b[at + h1el::COLOUR..at + h1el::COLOUR + 4].try_into().unwrap()),
    })
}

/// Halo 1's grenade HUD (`grhi`), from its draw (E `0x000db3c0`..`0x000db5c1`,
/// `esi` the `grhi` of the unit's selected grenade type, `[unit +0x2cc]`,
/// from the globals' grenades block `+0x12c`, `0x44` each, its reference
/// `+0x14`, datum `+0x20`).
pub mod grhi {
    /// The anchor (`u16`; the `grhi` is the header handed to the element
    /// draw, `0x000db4ed`).
    pub const ANCHOR: usize = 0x00;
    /// The background (`0x000db4ef`, drawn when its bitmap's datum
    /// `+0x54` is live).
    pub const BACKGROUND: usize = 0x24;
    /// The total-grenades background (`0x000db50f`, datum `+0xbc`).
    pub const TOTAL_BACKGROUND: usize = 0x8c;
    /// The count, a number element drawn by `0x000e1a60` when byte
    /// `+0x138` is set (`0x000db514`).
    pub const NUMBER: usize = 0xf4;
    pub const NUMBER_SHOWN: usize = 0x138;
    /// The count at or under which it flashes (`i16`, `0x000db494`).
    pub const LOW: usize = 0x148;
    /// The messaging part, drawn by `0x000e16b0` when the bitmap `+0x14c`
    /// is live (`0x000db54e`).
    pub const MESSAGING: usize = 0x14c;
    pub const SIZE: usize = 0x15c;
    /// In the Halo 1 globals (`matg`): the grenades block and an element's
    /// HUD reference.
    pub const GLOBALS_GRENADES: usize = 0x128;
    pub const GLOBALS_GRENADE_SIZE: usize = 0x44;
    pub const GLOBALS_GRENADE_HUD: usize = 0x14;
}

/// The drawable elements of the `grhi` `b`: background, then the total
/// background, as Halo 1 draws them.
pub fn grenade_elements(map: &H1Map, b: &[u8]) -> Result<Vec<Element>> {
    if b.len() < grhi::SIZE {
        return Err(Error::Truncated("grhi"));
    }
    let raw = u16::from_le_bytes([b[grhi::ANCHOR], b[grhi::ANCHOR + 1]]);
    let anchor =
        Anchor::from_halo1(raw).ok_or_else(|| Error::Invalid(format!("grhi anchor {raw}")))?;
    Ok([
        ("h1_grenade_background", grhi::BACKGROUND),
        ("h1_grenade_total_background", grhi::TOTAL_BACKGROUND),
    ]
    .into_iter()
    .filter_map(|(name, at)| element_at(map, b, at, name, Role::Static, anchor))
    .collect())
}

/// The state flags of a grenade type's widgets, by Halo 2's grenade
/// type index: those of Halo 2's own grenade backgrounds (V:
/// `ui\hud\masterchief` widgets 0 and 1, `grenade_bkd` sequences 0 and 1,
/// shown on `+0x08` word 0 bit `0x04` and `0x08`; its text widgets
/// `frag_count` and `plasma_count` have the same bits), so each shows only
/// while its type is the selected one, as Halo 1 draws only the selected
/// type's `grhi` (E `0x000db3f5`: `[unit +0x2cc]`).
pub const GRENADE_STATES: [[u8; 20]; 2] = [
    [0x04, 0, 0, 0, 0, 0, 0, 0, 0x2a, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0x08, 0, 0, 0, 0, 0, 0, 0, 0x26, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

/// Each Halo 1 grenade type's HUD (`grhi`), in the globals' order (the
/// type index), `None` for a type with none.
pub fn grenade_huds(map: &H1Map) -> Result<Vec<Option<H1TagId>>> {
    let Some(t) = map.tags().iter().find(|t| t.group == h1map::Group::new(b"matg")) else {
        return Ok(Vec::new());
    };
    let b = map.tag_data(t)?;
    Ok(crate::h1::elements(map, b, grhi::GLOBALS_GRENADES, grhi::GLOBALS_GRENADE_SIZE)?
        .into_iter()
        .map(|e| crate::h1::reference(map, e, grhi::GLOBALS_GRENADE_HUD))
        .collect())
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

/// Halo 1's weapon HUD (`wphi`) layout, from its draw (E, Halo 1's XBE,
/// `0x000db5d0`..`0x000dc0b3`, `ebx` the `wphi`; the crosshairs
/// `0x000dac68`..`0x000db374`).
pub mod wphi {
    /// The child `wphi` whose elements draw too (16-byte reference; its
    /// datum `+0x0c` is followed, `0x000dbded`).
    pub const CHILD: usize = 0x00;
    /// The anchor of the static elements, meters and numbers (`u16`; `+0x3c`
    /// is passed to the element draws as their header, `0x000dbe9a`).
    pub const ANCHOR: usize = 0x3c;
    /// Static elements (`0x000dbe4d`), `0xb4` each.
    pub const STATICS: usize = 0x60;
    /// Meters (`0x000dbeaf`), `0xb4` each.
    pub const METERS: usize = 0x6c;
    /// Numbers (`0x000dbf1e`), `0xa0` each.
    pub const NUMBERS: usize = 0x78;
    /// Crosshairs (`0x000dade0`), `0x68` each.
    pub const CROSSHAIRS: usize = 0x84;
    /// Overlay elements, `0x68` each.
    pub const OVERLAYS: usize = 0x90;
    /// The crosshair types any of the chain uses, a mask (`0x000da6c3`).
    pub const CROSSHAIR_TYPES: usize = 0x9c;
    pub const SIZE: usize = 0xa0;
    pub const STATIC_SIZE: usize = 0xb4;
    pub const METER_SIZE: usize = 0xb4;
    pub const NUMBER_SIZE: usize = 0xa0;
    pub const CROSSHAIR_SIZE: usize = 0x68;
    pub const OVERLAY_SIZE: usize = 0x68;

    /// A static, meter or number entry's head: the weapon state it reads
    /// (`i16`, an index into the states, `0x000dbe85`), its flags (bit 0:
    /// not drawn, `0x000dbe6f`), its screen type (a bit index tested
    /// against the screen mask, `0x000dbe74`); the element after it.
    pub mod entry {
        pub const STATE: usize = 0x00;
        pub const FLAGS: usize = 0x02;
        pub const SCREEN: usize = 0x04;
        pub const ELEMENT: usize = 0x24;
    }

    /// A crosshair: its type (`i16`, tested as a bit of the active types,
    /// `0x000dadf1`), screen type (`0x000dae12`), bitmap (reference, datum
    /// `+0x30`, `0x000daeba`) and overlays (block `+0x34`, `0x6c` each).
    pub mod crosshair {
        pub const TYPE: usize = 0x00;
        pub const SCREEN: usize = 0x04;
        pub const BITMAP: usize = 0x24;
        pub const OVERLAYS: usize = 0x34;
        pub const OVERLAY_SIZE: usize = 0x6c;
    }

    /// A crosshair overlay: offset (`i16` pair) and scale as an element's,
    /// colour `+0x24` (`0x000db0f2`), sequence `+0x46` (`0x000daed0`) and
    /// flags `+0x48` (bit 7: not drawn, `0x000dae63`; bit 1: no sprite,
    /// `0x000daeb2`; bit 4: spread with the weapon's error, `0x000db14d`).
    pub mod overlay {
        pub const OFFSET: usize = 0x00;
        pub const SCALE: usize = 0x04;
        pub const COLOUR: usize = 0x24;
        pub const SEQUENCE: usize = 0x46;
        pub const FLAGS: usize = 0x48;
    }

    /// A meter element's fill (E `0x000e20a0`): the state's value `v` (a
    /// byte) becomes `clamp(MULTIPLIER * v + OFFSET, 0, 255)`, raised to
    /// `MINIMUM`, the level the sprite's ramp is filled to.
    pub mod meter {
        pub const MINIMUM: usize = 0x45;
        pub const MULTIPLIER: usize = 0x48;
        pub const OFFSET: usize = 0x49;
    }
}

/// The weapon states Halo 1's weapon HUD reads (E `0x000db6e7`..: each
/// compared with its own low threshold, `wphi +0x14..+0x1a`, and the meters'
/// values set from them, `0x000dbc4e`..`0x000dbc99`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponState {
    /// 0: the rounds in reserve (weapon state `+0x12`, against `+0x14`).
    TotalAmmo,
    /// 1: the rounds loaded (`+0x0e`, against `+0x16`).
    LoadedAmmo,
    /// 2: the heat (`+0x00`, a fraction; 0..255 to the meters, constant
    /// `0x001f0aec`; times 100 against `+0x18`).
    Heat,
    /// 3: the battery, 1 - age (`+0x04`; 0..100 to the meters and against
    /// `+0x1a`, constant `0x001f0bec`).
    Battery,
    /// 4 and 5: the same two counts of the second trigger (`+0x1c`,
    /// `+0x18`).
    SecondTotal,
    SecondLoaded,
    Other(i16),
}

impl WeaponState {
    pub fn from_halo1(v: i16) -> Self {
        match v {
            0 => Self::TotalAmmo,
            1 => Self::LoadedAmmo,
            2 => Self::Heat,
            3 => Self::Battery,
            4 => Self::SecondTotal,
            5 => Self::SecondLoaded,
            n => Self::Other(n),
        }
    }
}

/// Halo 1's crosshair types (E `0x000da70a`, the table at `0x000daa18`
/// says when each is active). 0 is the aim crosshair (V: every weapon's
/// `hud_reticles` overlay); 1 the zoom level (`0x000da775`: the unit's
/// zoom level, the overlay's sprite chosen by it; V: the sniper rifle's
/// `hud_reticles_scope` sequence 1). The rest are warnings (V: their
/// overlays are `hud_reticle_warnings`).
pub mod crosshair_type {
    pub const AIM: i16 = 0;
    pub const ZOOM_LEVEL: i16 = 1;
}

/// One weapon HUD entry, as read.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponEntry {
    /// The `wphi` it is in (the weapon's own or a child).
    pub wphi: H1TagId,
    pub kind: WeaponEntryKind,
    pub screen: u8,
    pub hidden: bool,
    pub anchor: Anchor,
    /// Halo 1's offset (from the corner inward), scale, bitmap, sequence.
    pub offset: (i16, i16),
    pub scale: [f32; 2],
    pub bitmap: Option<H1TagId>,
    pub sequence: i16,
    /// Halo 1's colour (ARGB): an element's `+0x34`, an overlay's `+0x24`.
    pub colour: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WeaponEntryKind {
    Static(WeaponState),
    /// A meter: its state, and its fill's minimum, multiplier and offset.
    Meter(WeaponState, [u8; 3]),
    /// A crosshair overlay: its crosshair's type, the overlay's flags.
    Crosshair(i16, u32),
}

/// Every static, meter and crosshair overlay of the `wphi` `tag` and its
/// children, in Halo 1's draw order (the child first, `0x000dbded`), and
/// how many numbers and overlay elements the chain holds.
pub fn weapon_entries(map: &H1Map, tag: H1TagId) -> Result<(Vec<WeaponEntry>, usize)> {
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut seen = Vec::new();
    weapon_chain(map, tag, &mut out, &mut skipped, &mut seen)?;
    Ok((out, skipped))
}

fn weapon_chain(
    map: &H1Map,
    tag: H1TagId,
    out: &mut Vec<WeaponEntry>,
    skipped: &mut usize,
    seen: &mut Vec<H1TagId>,
) -> Result<()> {
    if seen.contains(&tag) {
        return Err(Error::Invalid("wphi chain loops".into()));
    }
    seen.push(tag);
    let t = map.tag(tag)?;
    let b = crate::h1::tag_bytes(map, t, b"wphi", wphi::SIZE)?;
    if let Some(child) = crate::h1::reference(map, b, wphi::CHILD) {
        weapon_chain(map, child, out, skipped, seen)?;
    }
    let raw = u16::from_le_bytes([b[wphi::ANCHOR], b[wphi::ANCHOR + 1]]);
    let anchor =
        Anchor::from_halo1(raw).ok_or_else(|| Error::Invalid(format!("wphi anchor {raw}")))?;
    let el = |e: &[u8], at: usize, seq: usize| {
        (
            (i16_at(e, at), i16_at(e, at + 2)),
            [f32_at(e, at + 4), f32_at(e, at + 8)],
            crate::h1::reference(map, e, at + h1el::BITMAP),
            i16_at(e, at + seq),
        )
    };
    let u32_at = |e: &[u8], at: usize| u32::from_le_bytes(e[at..at + 4].try_into().unwrap());
    use wphi::entry;
    for e in crate::h1::elements(map, b, wphi::STATICS, wphi::STATIC_SIZE)? {
        let (offset, scale, bitmap, sequence) = el(e, entry::ELEMENT, h1el::SEQUENCE);
        out.push(WeaponEntry {
            wphi: tag,
            kind: WeaponEntryKind::Static(WeaponState::from_halo1(i16_at(e, entry::STATE))),
            screen: e[entry::SCREEN],
            hidden: e[entry::FLAGS] & 1 != 0,
            anchor,
            offset,
            scale,
            bitmap,
            sequence,
            colour: u32_at(e, entry::ELEMENT + h1el::COLOUR),
        });
    }
    for e in crate::h1::elements(map, b, wphi::METERS, wphi::METER_SIZE)? {
        let (offset, scale, bitmap, sequence) = el(e, entry::ELEMENT, h1el::METER_SEQUENCE);
        let m = entry::ELEMENT;
        out.push(WeaponEntry {
            wphi: tag,
            kind: WeaponEntryKind::Meter(
                WeaponState::from_halo1(i16_at(e, entry::STATE)),
                [
                    e[m + wphi::meter::MINIMUM],
                    e[m + wphi::meter::MULTIPLIER],
                    e[m + wphi::meter::OFFSET],
                ],
            ),
            screen: e[entry::SCREEN],
            hidden: e[entry::FLAGS] & 1 != 0,
            anchor,
            offset,
            scale,
            bitmap,
            sequence,
            colour: u32_at(e, entry::ELEMENT + h1el::COLOUR),
        });
    }
    use wphi::{crosshair, overlay};
    for c in crate::h1::elements(map, b, wphi::CROSSHAIRS, wphi::CROSSHAIR_SIZE)? {
        let bitmap = crate::h1::reference(map, c, crosshair::BITMAP);
        for o in crate::h1::elements(map, c, crosshair::OVERLAYS, crosshair::OVERLAY_SIZE)? {
            let flags = u32::from_le_bytes(o[overlay::FLAGS..overlay::FLAGS + 4].try_into().unwrap());
            out.push(WeaponEntry {
                wphi: tag,
                kind: WeaponEntryKind::Crosshair(i16_at(c, crosshair::TYPE), flags),
                screen: c[crosshair::SCREEN],
                hidden: flags & 0x80 != 0,
                // Crosshairs are placed from the centre (E `0x000dad9e`: the
                // header handed to their draw holds anchor 4).
                anchor: Anchor::Centre,
                offset: (i16_at(o, overlay::OFFSET), i16_at(o, overlay::OFFSET + 2)),
                scale: [f32_at(o, overlay::SCALE), f32_at(o, overlay::SCALE + 4)],
                bitmap,
                sequence: i16_at(o, overlay::SEQUENCE),
                colour: u32_at(o, overlay::COLOUR),
            });
        }
    }
    *skipped += crate::h1::elements(map, b, wphi::NUMBERS, wphi::NUMBER_SIZE)?.len()
        + crate::h1::elements(map, b, wphi::OVERLAYS, wphi::OVERLAY_SIZE)?.len();
    Ok(())
}

/// A crosshair's state flags, shown zoomed or not: Halo 2's battle rifle
/// reticle's (V: `ui\hud\battle_rifle` widget 3, `hud_reticles`, which
/// stays in the scope).
pub const AIM_STATE: [u8; 20] = [
    0x30, 0, 0, 0, 0x03, 0, 0, 0, 0, 0, 0x11, 0, 0x04, 0, 0, 0, 0, 0, 0, 0,
];

/// A zoomed-only crosshair's state flags: Halo 2's scope masks' (V:
/// `ui\hud\sniper_rifle` and `battle_rifle` widget 0, the scope masks,
/// `+0x08` bit `0x40`).
pub const ZOOM_STATE: [u8; 20] = [
    0x90, 0x01, 0, 0, 0x01, 0, 0, 0, 0x40, 0, 0, 0, 0x04, 0, 0, 0, 0, 0, 0, 0,
];

/// Crosshair overlay flags (`wphi` crosshair overlay `+0x48`): bit 2 draws
/// only while the type's value is above 0, bit 6 only while it is 0
/// (`0x000dae69`, `0x000dae7c`). On the aim type that value is the zoom
/// (V: Halo 1's sniper rifle's scope lines, `sniper_scope_crosshairs2`,
/// have bit 2).
pub const OVERLAY_WHEN_ZOOMED: u32 = 0x04;
pub const OVERLAY_WHEN_UNZOOMED: u32 = 0x40;

/// The suffix of a converted Halo 1 weapon HUD's name.
pub const WEAPON_NHDT_SUFFIX: &str = "_h1_weapon_hud";

/// Halo 1's screen types an entry is drawn on (`entry +4`, a bit index of
/// the mask built at `0x000dbe16`): bit 1 is set with one local player,
/// bit 2 with more (split screen), bit 0 unless the engine's mode
/// `[0x0039be5c] +0x3c` is 2. The converter keeps entries for 0 and 1,
/// a full screen; split-screen-only ones (2) are left out.
pub const SPLIT_SCREEN_ONLY: u8 = 2;

/// The Halo 2 widget input that reads what a Halo 1 meter's state does
/// (Halo 2's input switch `0x0013ad48`, its weapon block in `ecx`):
/// rounds loaded over the magazine's (`0x36`, `[ecx+6]/[ecx+0x0a]`,
/// `0x0013ae7c`; V: Halo 2's magnum ammo meter), the battery (`0x38`,
/// `1 - [ecx+0x10]`, `0x0013af94`; V: the energy sword's), the heat
/// (`0x31`, `[ecx+0x14]`, `0x0013af00`; V: the beam rifle's and sentinel
/// beam's meters, Halo 2's heat weapons). Halo 1 fills to
/// `multiplier * v + offset` of 255 (`wphi::meter`): on the disc's meters
/// that is the same fraction (V: sniper rifle 60 per round of 4 plus 1,
/// plasma heat 1 per 255th).
pub fn weapon_meter_input(s: WeaponState) -> Option<u8> {
    match s {
        WeaponState::LoadedAmmo => Some(0x36),
        WeaponState::Battery => Some(0x38),
        WeaponState::Heat => Some(0x31),
        _ => None,
    }
}

/// Why a weapon HUD entry has no widget.
#[derive(Clone, Debug, PartialEq)]
pub enum Skip {
    /// Its flags say not drawn.
    Hidden,
    /// Drawn on a split screen only ([`SPLIT_SCREEN_ONLY`]).
    SplitScreen,
    /// No bitmap, or nothing in its sequence.
    NothingDrawn,
    /// A meter on a state no Halo 2 input reads.
    MeterState(WeaponState),
    /// A crosshair of a type not converted.
    CrosshairType(i16),
    /// An aim overlay drawn only unzoomed: no Halo 2 state flags for it
    /// are established.
    UnzoomedOnly,
    /// Scaled ([`unscaled`]).
    Scaled([f32; 2]),
}

/// The widget for weapon HUD entry `e` (its bitmap converted as
/// `bitmap`), or why it has none. Statics and meters keep Halo 1's anchor,
/// offset turned to screen directions and corner registration, as the unit
/// HUD's; crosshairs are centred on the screen and the sprite (E
/// `0x000dad9e`: their header's anchor is 4; `0x000e0330` centres the
/// sprite on anchor 4).
pub fn weapon_widget(
    e: &WeaponEntry,
    bitmap: Option<TagId>,
    meter: TagId,
    plain: TagId,
) -> std::result::Result<Widget, Skip> {
    if e.hidden {
        return Err(Skip::Hidden);
    }
    if e.screen == SPLIT_SCREEN_ONLY {
        return Err(Skip::SplitScreen);
    }
    if !unscaled(e.scale) {
        return Err(Skip::Scaled(e.scale));
    }
    let bitmap = bitmap.ok_or(Skip::NothingDrawn)?;
    let (name, inputs, shader, state) = match &e.kind {
        WeaponEntryKind::Static(_) => ("h1_weapon_static", [0; 4], plain, STATE),
        WeaponEntryKind::Meter(s, _) => {
            let input = weapon_meter_input(*s).ok_or(Skip::MeterState(*s))?;
            ("h1_weapon_meter", [input, 0, 0, 0], meter, STATE)
        }
        WeaponEntryKind::Crosshair(t, flags) => match *t {
            crosshair_type::AIM if flags & OVERLAY_WHEN_UNZOOMED != 0 => {
                return Err(Skip::UnzoomedOnly);
            }
            crosshair_type::AIM if flags & OVERLAY_WHEN_ZOOMED != 0 => {
                ("h1_scope", [0; 4], plain, ZOOM_STATE)
            }
            crosshair_type::AIM => ("h1_crosshair", [0; 4], plain, AIM_STATE),
            t => return Err(Skip::CrosshairType(t)),
        },
    };
    Ok(Widget {
        name: name.to_owned(),
        inputs,
        state,
        anchor: e.anchor,
        flags: 0,
        bitmap,
        shader,
        sequence: e.sequence.clamp(0, i8::MAX as i16) as i8,
        offset: to_halo2_offset(e.anchor, e.offset),
        registration: registration(e.anchor),
        special: 0,
    })
}

/// Whether Halo 1 draws an element at its sprite's own size. Halo 1
/// multiplies the sprite's size and the offset by the element's scale
/// (`+4`, two `f32`; E `0x000e15f4`, `0x000e15ff`). A Halo 2
/// bitmap widget has no scale: it draws the sprite at its size. Every
/// element on the disc is unscaled (V: `hud_disc` `grenade_huds_read`, the
/// unit and grenade elements of every map and the weapon entries of
/// bloodgulch and a10, all 1.0), so Halo 1's size is kept; a scaled one
/// is refused rather than drawn at the wrong size.
pub fn unscaled(scale: [f32; 2]) -> bool {
    scale == [1.0, 1.0]
}

/// The alpha of Halo 1's element colour (ARGB `+0x34`).
pub fn colour_alpha(colour: u32) -> u8 {
    (colour >> 24) as u8
}

/// Halo 1's pixels with every texel's alpha multiplied by `alpha` (of
/// 255), as Halo 1's draw multiplies the sprite by its element's colour,
/// alpha included (E `0x000e1830`: the colour `+0x34`). Halo 2's HUD shader
/// keeps the texture's alpha (`widget_gradient_flash`: `a = t0.a * c0.a`)
/// but takes no colour from the widget, so the colour's alpha goes into
/// the sprite. Every texel's alpha scales alike, so the Xbox swizzle does
/// not matter. `None` for a format whose alpha is not a byte of its own
/// (block-compressed, 16-bit, no alpha).
pub fn scale_alpha(format: h1map::bitmap::PixelFormat, px: &mut [u8], alpha: u8) -> Option<()> {
    use h1map::bitmap::PixelFormat as F;
    let (stride, at) = match format {
        F::A8 => (1, 0),
        // Y then A (h1bitmaps decode).
        F::A8Y8 => (2, 1),
        // B, G, R, A.
        F::A8R8G8B8 => (4, 3),
        _ => return None,
    };
    for t in px.chunks_exact_mut(stride) {
        t[at] = ((u16::from(t[at]) * u16::from(alpha) + 127) / 255) as u8;
    }
    Some(())
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

    fn entry(kind: WeaponEntryKind) -> WeaponEntry {
        WeaponEntry {
            wphi: H1TagId(0),
            kind,
            screen: 0,
            hidden: false,
            anchor: Anchor::TopLeft,
            offset: (6, 21),
            scale: [1.0, 1.0],
            bitmap: Some(H1TagId(1)),
            sequence: 6,
            colour: 0,
        }
    }

    #[test]
    fn weapon_meters_read_halo2s_weapon_inputs() {
        let m = entry(WeaponEntryKind::Meter(WeaponState::LoadedAmmo, [0, 60, 1]));
        let w = weapon_widget(&m, Some(TagId(1)), TagId(2), TagId(3)).unwrap();
        assert_eq!(w.inputs, [0x36, 0, 0, 0]);
        assert_eq!(w.shader, TagId(2));
        assert_eq!((w.anchor, w.offset, w.registration), (Anchor::TopLeft, (6, 21), [0.0, 0.0]));
        assert_eq!(w.state, STATE);
        let heat = entry(WeaponEntryKind::Meter(WeaponState::Heat, [0, 1, 0]));
        assert_eq!(weapon_widget(&heat, Some(TagId(1)), TagId(2), TagId(3)).unwrap().inputs[0], 0x31);
        let battery = entry(WeaponEntryKind::Meter(WeaponState::Battery, [0, 1, 0]));
        assert_eq!(weapon_widget(&battery, Some(TagId(1)), TagId(2), TagId(3)).unwrap().inputs[0], 0x38);
        let total = entry(WeaponEntryKind::Meter(WeaponState::TotalAmmo, [0, 1, 0]));
        assert_eq!(
            weapon_widget(&total, Some(TagId(1)), TagId(2), TagId(3)),
            Err(Skip::MeterState(WeaponState::TotalAmmo))
        );
    }

    #[test]
    fn crosshairs_are_centred_and_scope_lines_show_zoomed() {
        let mut c = entry(WeaponEntryKind::Crosshair(crosshair_type::AIM, 1));
        c.anchor = Anchor::Centre;
        c.offset = (-30, 30);
        let w = weapon_widget(&c, Some(TagId(1)), TagId(2), TagId(3)).unwrap();
        assert_eq!((w.anchor, w.registration), (Anchor::Centre, [0.5, 0.5]));
        assert_eq!(w.offset, (-30, 30), "centre: no sign change");
        assert_eq!((w.state, w.shader, w.inputs), (AIM_STATE, TagId(3), [0; 4]));
        c.kind = WeaponEntryKind::Crosshair(crosshair_type::AIM, 6);
        assert_eq!(weapon_widget(&c, Some(TagId(1)), TagId(2), TagId(3)).unwrap().state, ZOOM_STATE);
        c.kind = WeaponEntryKind::Crosshair(crosshair_type::AIM, 0x40);
        assert_eq!(weapon_widget(&c, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::UnzoomedOnly));
        c.kind = WeaponEntryKind::Crosshair(9, 1);
        assert_eq!(weapon_widget(&c, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::CrosshairType(9)));
    }

    #[test]
    fn hidden_split_screen_and_empty_entries_have_no_widget() {
        let mut s = entry(WeaponEntryKind::Static(WeaponState::TotalAmmo));
        assert!(weapon_widget(&s, Some(TagId(1)), TagId(2), TagId(3)).is_ok());
        assert_eq!(weapon_widget(&s, None, TagId(2), TagId(3)), Err(Skip::NothingDrawn));
        s.screen = SPLIT_SCREEN_ONLY;
        assert_eq!(weapon_widget(&s, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::SplitScreen));
        s.screen = 1;
        s.hidden = true;
        assert_eq!(weapon_widget(&s, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::Hidden));
    }

    #[test]
    fn the_colours_alpha_scales_the_sprites_alpha() {
        use h1map::bitmap::PixelFormat as F;
        let mut px = vec![0x10, 0xff, 0x20, 0x80];
        scale_alpha(F::A8Y8, &mut px, 0x28).unwrap();
        assert_eq!(px, [0x10, 0x28, 0x20, 0x14], "luminance kept, alpha times 0x28/255");
        let mut px = vec![1, 2, 3, 0xff];
        scale_alpha(F::A8R8G8B8, &mut px, 0xa0).unwrap();
        assert_eq!(px, [1, 2, 3, 0xa0]);
        assert!(scale_alpha(F::Dxt5, &mut [0; 16], 0x28).is_none());
        assert_eq!(colour_alpha(0x2806_1428), 0x28);
    }

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
            colour: 0,
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
