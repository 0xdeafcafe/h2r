//! Halo 1's unit HUD (`unhi`) as a Halo 2 HUD (`nhdt`), written from
//! scratch: every byte of the `nhdt` and its widgets is set here, none is a
//! template's (docs/halo1/hud.md).
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
    /// The motion sensor's placement: an offset (two `i16`) and scale (two
    /// `f32`), as an element's, placed by `0x000e07a0` from the sensor's
    /// stand-in anchor (bottom left, `0x000dd5ab`); the sweep and blips are
    /// drawn around that point (E `0x000dd658`..`0x000dd691`, `0x000da1e0`
    /// reads the point).
    pub const MOTION_SENSOR: usize = 0x35c;
    /// The root's size (V: every `unhi` is at least this long, the
    /// cyborg's with its blocks after it).
    pub const SIZE: usize = 0x56c;
}

/// Where Halo 1 centres the motion sensor's sweep and blips (the
/// placement [`unhi::MOTION_SENSOR`] from the bottom-left corner), as a
/// Halo 2 offset from the same corner: what Halo 2's blip centre table
/// (`0x47ff40`, anchor 2) holds, (41, -42) for its own sensor in
/// fullscreen. A scaled placement is refused, as for the elements.
pub fn sensor_centre(b: &[u8]) -> Result<[i16; 2]> {
    let at = unhi::MOTION_SENSOR;
    if b.len() < at + 12 {
        return Err(Error::Truncated("unhi"));
    }
    let scale = [f32_at(b, at + 4), f32_at(b, at + 8)];
    if !unscaled(scale) {
        return Err(Error::Invalid(format!("motion sensor scaled {scale:?}")));
    }
    let (x, y) = to_halo2_offset(Anchor::BottomLeft, (i16_at(b, at), i16_at(b, at + 2)));
    Ok([x, y])
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
/// up from a bottom anchor, docs/halo1/health.md).
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
    /// A number entry's flags byte: bit 0 shows the value in magazines
    /// (`0x000dbf5f`).
    pub const NUMBER_MAGAZINES: usize = 0x78;
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
    /// A number (`wphi +0x78`, drawn by the number draw `0x000e1a60` as the
    /// grenade count is, E `0x000dbf1e`..`0x000dc0f0`): the state whose
    /// value it shows, the number element (its anchor the `wphi`'s, its
    /// offset Halo 1's), and whether it shows that value in magazines
    /// (entry `+0x78` bit 0: divided by the weapon's magazine size,
    /// `0x000dbf5f`..`0x000dbf77`, `0x000dc0de`).
    Number(WeaponState, Number, bool),
}

/// Every static, meter, number and crosshair overlay of the `wphi` `tag`
/// and its children, in Halo 1's draw order (the child first,
/// `0x000dbded`), and how many overlay elements the chain holds.
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
    for e in crate::h1::elements(map, b, wphi::NUMBERS, wphi::NUMBER_SIZE)? {
        let at = entry::ELEMENT;
        let number = Number {
            anchor,
            offset: (i16_at(e, at), i16_at(e, at + 2)),
            scale: [f32_at(e, at + 4), f32_at(e, at + 8)],
            digits: e[at + number::DIGITS],
            flags: e[at + number::FLAGS],
            fraction_digits: e[at + number::FRACTION_DIGITS],
        };
        out.push(WeaponEntry {
            wphi: tag,
            kind: WeaponEntryKind::Number(
                WeaponState::from_halo1(i16_at(e, entry::STATE)),
                number,
                e[wphi::NUMBER_MAGAZINES] & 1 != 0,
            ),
            screen: e[entry::SCREEN],
            hidden: e[entry::FLAGS] & 1 != 0,
            anchor,
            offset: number.offset,
            scale: number.scale,
            bitmap: None,
            sequence: 0,
            colour: u32_at(e, at + number::COLOUR),
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
    *skipped += crate::h1::elements(map, b, wphi::OVERLAYS, wphi::OVERLAY_SIZE)?.len();
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
    /// A number: a text widget ([`weapon_number`]), not a bitmap one.
    Text,
    /// A number on a state no Halo 2 number input reads.
    NumberState(WeaponState),
    /// A number shown in magazines: Halo 2's text has no divide.
    Magazines,
    /// A number with fraction digits.
    Fraction,
    /// A static whose colour is all zero: Halo 1 draws nothing of it
    /// ([`invisible_colour`]).
    Invisible,
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
    if matches!(e.kind, WeaponEntryKind::Number(..)) {
        return Err(Skip::Text);
    }
    if !unscaled(e.scale) {
        return Err(Skip::Scaled(e.scale));
    }
    if matches!(e.kind, WeaponEntryKind::Static(_)) && static_alpha(e.colour).is_none() {
        return Err(Skip::Invisible);
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
        WeaponEntryKind::Number(..) => unreachable!("refused above"),
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

/// The alpha a converted static element's sprite carries
/// ([`scale_alpha`]) for Halo 1's element colour `colour`, `None` when
/// Halo 1 draws nothing of it.
///
/// Halo 1's element draw (E `0x000e1320`, the draw struct's `+0x88` =
/// 7) blends in Halo 1's mode 7, whose source factor is ONE, its
/// destination factor ONE_MINUS_SRC_ALPHA and its operation ADD (E
/// `0x0007f4e0`: the tables at `0x001e9c0c`, `0x001e9c30`, `0x001e9c54`,
/// entry 7: `1`, `0x303`, `0x8006`, the D3D blend values). It writes the
/// element's colour into every vertex's diffuse (`0x000e1320`, `+0x0c`);
/// the texture is multiplied by it (inferred: the vertex layout, not the
/// texture stage setup, was read). So what lands is `tex * colour +
/// dst * (1 - tex.a * colour.a)`:
///
/// - colour `0x00000000` adds nothing and keeps all of the destination:
///   the element is not drawn at all. Halo 1's weapon HUDs give one such
///   static to every ballistic weapon (V: bloodgulch, `hud_ammo_alphas`
///   on the assault rifle, pistol, shotgun, rocket launcher, sniper rifle
///   and needler, on the "rounds loaded" state), which drew opaque in v66.
/// - alpha 0 with a colour adds the sprite's colour over the destination
///   (an additive glow): drawn at full alpha, the nearest Halo 2's
///   `widget_gradient_flash` has (it blends by alpha only).
/// - any other alpha scales the sprite's alpha (as before).
pub fn static_alpha(colour: u32) -> Option<u8> {
    match (colour, colour_alpha(colour)) {
        (0, _) => None,
        (_, 0) => Some(0xff),
        (_, a) => Some(a),
    }
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

/// What Halo 2's legacy meter shader (`widget_legacy_meter`, the
/// [`METER_SHADER`] the converted meters draw through) shows of a texel:
/// its alpha plus, on the filled side, its green, at most 1.
///
/// The shader's combiners (V: `ui\hud\shaders\ammo_meter`'s pass, the
/// pixel shader definition `h2 shader` prints): stage 0 takes `t2 =
/// c0 . t0.rgb` with stage 0's `c0` fixed at `0x0000ff00` (no engine
/// constant writes it: `c0` written mask `0x2e`), so `t2` is the texel's
/// green; stage 1 sets `r0.a` from `hud_input1` less the blue, whose sign
/// picks the filled side; stage 3 takes `t1.a` from stage 3's `c1`, fixed
/// at `0xffffffff`, on the filled side; stage 4 makes the alpha `t0.a +
/// t1.a * t2`. So a filled texel's alpha is its own plus its green.
/// Halo 1's meter draw shows the alpha alone: a texel Halo 1 leaves clear
/// shows on Halo 2's filled side when its colour is not black.
pub fn legacy_meter_alpha(rgba: [u8; 4], filled: bool) -> u8 {
    let green = if filled { rgba[1] } else { 0 };
    rgba[3].saturating_add(green)
}

/// A Halo 1 meter sprite's pixels with the colour of every clear texel
/// (alpha 0) made black, so the legacy meter shader leaves them clear as
/// Halo 1 does ([`legacy_meter_alpha`]). Halo 1's meter sprites carry
/// their luminance ramp under the clear parts too (V: `hud_unit_meters`,
/// the cyborg's shield meter has 1435 clear texels with a luminance, up to
/// 254, 722 of them on the health meter's texels as Halo 1 lays the two
/// out). A visible texel is unchanged. `None` for a format whose texels
/// are not a byte each of alpha and colour (block-compressed, 16-bit, no
/// alpha); `AY8` and `A8` have no colour of their own and are left as
/// they are.
pub fn clear_meter_colour(format: h1map::bitmap::PixelFormat, px: &mut [u8]) -> Option<()> {
    use h1map::bitmap::PixelFormat as F;
    let (stride, alpha, colour): (usize, usize, &[usize]) = match format {
        F::A8 | F::Ay8 => return Some(()),
        // Y then A (h1bitmaps decode).
        F::A8Y8 => (2, 1, &[0]),
        // B, G, R, A.
        F::A8R8G8B8 => (4, 3, &[0, 1, 2]),
        _ => return None,
    };
    for t in px.chunks_exact_mut(stride) {
        if t[alpha] == 0 {
            for c in colour {
                t[*c] = 0;
            }
        }
    }
    Some(())
}

/// A Halo 1 HUD image whose format has no alpha byte of its own (the
/// motion sensor's sheets are DXT, `hud_weapon_backgrounds` and
/// `hud_unit_backgrounds`), decoded with Halo 2's decoder and rewritten
/// as a linear `A8R8G8B8` image with every texel's alpha multiplied by
/// `alpha`, as [`scale_alpha`] does for byte-alpha formats. Only the top
/// level is kept (a linear image has no mip levels; HUD sprites draw at
/// their own size). Retail Halo 2 HUD bitmaps are linear `A8R8G8B8` too
/// (E: `grenade_bkd`'s images, flags `linear | 0x290`). Rows are padded to
/// Halo 2's linear pitch (64 bytes). 2D images only.
pub fn alpha_scaled_argb(
    img: &h1map::bitmap::Image,
    pixels: &[u8],
    alpha: u8,
) -> h1bitmaps::Result<(h1map::bitmap::Image, Vec<u8>)> {
    use h1map::bitmap::{self as b, ImageKind, PixelFormat};
    if img.kind != ImageKind::D2 {
        return Err(h1bitmaps::Error::Unsupported(format!("HUD image type {:?}", img.kind)));
    }
    let levels = h1bitmaps::decode_image(img, pixels)?;
    let top = levels
        .iter()
        .find(|l| l.face == 0 && l.level == 0)
        .ok_or_else(|| h1bitmaps::Error::Invalid("no top level".into()))?;
    let mut argb = img.clone();
    argb.format = PixelFormat::A8R8G8B8;
    argb.flags = (img.flags & !(b::FLAG_COMPRESSED | b::FLAG_PALETTIZED | b::FLAG_SWIZZLED))
        | b::FLAG_LINEAR;
    argb.mip_count = 0;
    argb.depth = 1;
    let (w, h) = (top.width as usize, top.height as usize);
    // The row pitch Halo 2's descriptor gives a linear image (64-byte rows).
    let pitch = h1bitmaps::texture_desc(&argb)?.pitch as usize;
    let mut out = vec![0u8; h1bitmaps::decode::layout_len(&argb)?.max(pitch * h)];
    for y in 0..h {
        for x in 0..w {
            let s = &top.rgba[(y * w + x) * 4..][..4];
            let a = ((u16::from(s[3]) * u16::from(alpha) + 127) / 255) as u8;
            out[y * pitch + x * 4..][..4].copy_from_slice(&[s[2], s[1], s[0], a]);
        }
    }
    argb.pixel_size = out.len() as u32;
    Ok((argb, out))
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

/// A rectangle in HUD pixels from the anchor's point, y down: left, top,
/// right, bottom.
pub type Rect = [i32; 4];

/// The size in pixels of what `bm`'s sequence `seq` draws ([`sprite`]):
/// the sprite's bounds times its image's size, as both games take it (E
/// Halo 1 `0x000e0330`: the bounds' spans times the image's width and
/// height; Halo 2 `0x22b1b5`..`0x22b28f`: the same spans times the
/// image's `+4`, `+6`).
pub fn sprite_size(bm: &h1map::bitmap::Bitmap, seq: i16) -> Option<(i32, i32)> {
    let (b, _) = sprite(bm, seq)?;
    let s = bm.sequences.get(usize::try_from(seq).ok()?)?;
    let image = s.sprites.first().map_or(s.first_image, |p| p.image);
    let img = bm.images.get(usize::try_from(image).ok()?)?;
    Some((
        ((b[1] - b[0]) * f32::from(img.width)).round() as i32,
        ((b[3] - b[2]) * f32::from(img.height)).round() as i32,
    ))
}

/// Where Halo 1 draws an element `size` pixels big with offset `offset`
/// (Halo 1's, inward from the corner) from its anchor's point. E, Halo 1's
/// XBE: `0x000e07a0` places the point, x times -1 on a right anchor and y
/// on a bottom one; `0x000e0330` lays the sprite from it, `[0, w]` across
/// on a left anchor, `[-w, 0]` on a right one, `[0, h]` down on a top
/// anchor, `[-h, 0]` on a bottom one, centred on the centre.
pub fn halo1_rect(anchor: Anchor, offset: (i16, i16), (w, h): (i32, i32)) -> Rect {
    let x = i32::from(if matches!(anchor, Anchor::TopRight | Anchor::BottomRight) {
        -offset.0
    } else {
        offset.0
    });
    let y = i32::from(if matches!(anchor, Anchor::BottomLeft | Anchor::BottomRight) {
        -offset.1
    } else {
        offset.1
    });
    let (x0, x1) = match anchor {
        Anchor::TopLeft | Anchor::BottomLeft => (x, x + w),
        Anchor::TopRight | Anchor::BottomRight => (x - w, x),
        Anchor::Centre => (x - w / 2, x - w / 2 + w),
    };
    let (y0, y1) = match anchor {
        Anchor::TopLeft | Anchor::TopRight => (y, y + h),
        Anchor::BottomLeft | Anchor::BottomRight => (y - h, y),
        Anchor::Centre => (y - h / 2, y - h / 2 + h),
    };
    [x0, y0, x1, y1]
}

/// Where Halo 2 draws a bitmap widget's sprite, `size` pixels big, from
/// its anchor's point (E `0x22aff1`): the point plus the widget's offset
/// for the screen mode (`+0x34`) is where the registration point lands
/// (`+0x40`, fractions of the sprite: `left + (right - left) * r`); the
/// corners lie `(bound - registration) * image size` from it.
pub fn halo2_rect(w: &Widget, (sw, sh): (i32, i32)) -> Rect {
    let x0 = i32::from(w.offset.0) - (w.registration[0] * sw as f32) as i32;
    let y0 = i32::from(w.offset.1) - (w.registration[1] * sh as f32) as i32;
    [x0, y0, x0 + sw, y0 + sh]
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

/// Halo 2's text widget, `0x54` bytes in the `nhdt`'s `+0x10` block (V:
/// the retail `ui\hud\masterchief` `frag_count` and `plasma_count`; E the
/// draw `0x22aa16`, its string `0x22a871`, its caller `0x13a63d`, which
/// tests the state at `+0x08` with `0x13ac87` and reads the inputs at
/// `+0x04` with `0x13ad48`, as for a bitmap widget).
pub mod text {
    /// Name (string ID), for the debug text only.
    pub const NAME: usize = 0x00;
    /// Four input bytes; their values are the four `hud_input<n>` numbers
    /// (`0x22a89d` copies them to `0x4e6980`).
    pub const INPUTS: usize = 0x04;
    /// State flags, as a bitmap widget's (`+0x08`..`+0x1b`).
    pub const STATE: usize = 0x08;
    /// Anchor (`u16`, `0x13a690`, `0x1396c7`).
    pub const ANCHOR: usize = 0x1c;
    /// Flags (`u16`, `0x22a8a4`): [`super::TEXT_NUMBER`] draws the number
    /// [`STRING`] names; `2` pads it to two digits, `4` to three; `8` draws
    /// the player's name. Otherwise `STRING` is a text to look up.
    pub const FLAGS: usize = 0x1e;
    /// The shader (tag reference; `0x22aa2f` skips the widget without one).
    pub const SHADER: usize = 0x20;
    /// String ID: with [`super::TEXT_NUMBER`] the value it names
    /// (`0x22ace7`: `hud_input1`..`hud_input4` the inputs `+0x04`..`+0x07`).
    pub const STRING: usize = 0x28;
    /// Justification (`u16`, `0x22ac0b`): 0 the text starts at the point, 1
    /// is centred on it, 2 ends at it.
    pub const JUSTIFICATION: usize = 0x2c;
    /// The font for fullscreen, half and quarter screen (a byte each,
    /// E `0x22a9a4`..`0x22a9ae`; [`super::TEXT_FONT`]).
    pub const FONTS: usize = 0x30;
    /// Offsets (two `i16`) for fullscreen, half and quarter screen
    /// (`0x22ab46`, `0x22ab38`, `0x22ab2a`).
    pub const OFFSETS: [usize; 3] = [0x40, 0x44, 0x48];
    pub const SIZE: usize = 0x54;
}

/// The `nhdt`'s text widgets (count, address).
pub const NHDT_TEXT_WIDGETS: usize = 0x10;
/// [`text::FLAGS`]: the widget draws a number.
pub const TEXT_NUMBER: u16 = 1;
/// [`text::JUSTIFICATION`]: the text ends at the point.
pub const JUSTIFY_RIGHT: u16 = 2;
/// The value a number widget draws: its first input (`0x22ace7`).
pub const TEXT_INPUT1: &str = "hud_input1";
/// Halo 2's grenade counts' shader (V: `frag_count`, `plasma_count`).
pub const TEXT_SHADER: &str = "ui\\hud\\shaders\\text_flash";
/// `0x13ad48`: input 20 is the frag count (HUD state `+0x38`), 21 the
/// plasma count (`+0x3a`); by Halo 2's grenade type index.
pub const INPUT_GRENADES: u8 = 20;

/// A Halo 2 text widget.
#[derive(Clone, Debug, PartialEq)]
pub struct TextWidget {
    pub name: String,
    pub inputs: [u8; 4],
    pub state: [u8; 20],
    pub anchor: Anchor,
    pub flags: u16,
    pub shader: TagId,
    /// [`text::STRING`].
    pub string: &'static str,
    pub justification: u16,
    pub offset: (i16, i16),
}

impl TextWidget {
    /// The widget's bytes; the name, shader and string are fixups.
    pub fn bytes(&self) -> Vec<u8> {
        let mut w = vec![0u8; text::SIZE];
        w[text::INPUTS..text::INPUTS + 4].copy_from_slice(&self.inputs);
        w[text::STATE..text::STATE + 20].copy_from_slice(&self.state);
        w[text::ANCHOR..text::ANCHOR + 2].copy_from_slice(&(self.anchor as u16).to_le_bytes());
        w[text::FLAGS..text::FLAGS + 2].copy_from_slice(&self.flags.to_le_bytes());
        w[text::JUSTIFICATION..text::JUSTIFICATION + 2]
            .copy_from_slice(&self.justification.to_le_bytes());
        w[text::FONTS..text::FONTS + 3].fill(TEXT_FONT);
        for at in text::OFFSETS {
            w[at..at + 2].copy_from_slice(&self.offset.0.to_le_bytes());
            w[at + 2..at + 4].copy_from_slice(&self.offset.1.to_le_bytes());
        }
        w
    }
}

/// A number element's fields after the element head (offset, scale):
/// digits, flags, fraction digits (E `0x000e1a60`, `ebx` the element:
/// `+0x44`, `+0x45`, `+0x46`).
pub mod number {
    /// The colour (ARGB): the number draw's default (`ebx+0x24`,
    /// `0x000e1c85`; `+0x3c` disabled, `0x000e1c67`). A number element has
    /// no bitmap reference, so its colour stands where a bitmap element's
    /// reference does.
    pub const COLOUR: usize = 0x24;
    pub const DIGITS: usize = 0x44;
    pub const FLAGS: usize = 0x45;
    pub const FRACTION_DIGITS: usize = 0x46;
}

/// Halo 1's grenade count, the number element `grhi +0xf4` (E the number
/// draw `0x000e1a60`, `ebx` the element): offset `+0x00` and scale
/// `+0x04` as any element's, the digits `+0x44` (byte), its flags
/// `+0x45` (bit `0x04` one digit more) and fraction digits `+0x46`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Number {
    pub anchor: Anchor,
    /// Halo 1's offset, inward from the anchor's corner.
    pub offset: (i16, i16),
    pub scale: [f32; 2],
    pub digits: u8,
    pub flags: u8,
    pub fraction_digits: u8,
}

/// The grenade count of the `grhi` `b`, `None` when Halo 1 does not draw
/// it (byte `+0x138` clear, `0x000db514`).
pub fn grenade_number(b: &[u8]) -> Result<Option<Number>> {
    if b.len() < grhi::SIZE {
        return Err(Error::Truncated("grhi"));
    }
    if b[grhi::NUMBER_SHOWN] == 0 {
        return Ok(None);
    }
    let raw = u16::from_le_bytes([b[grhi::ANCHOR], b[grhi::ANCHOR + 1]]);
    let anchor =
        Anchor::from_halo1(raw).ok_or_else(|| Error::Invalid(format!("grhi anchor {raw}")))?;
    let at = grhi::NUMBER;
    Ok(Some(Number {
        anchor,
        offset: (i16_at(b, at), i16_at(b, at + 2)),
        scale: [f32_at(b, at + 4), f32_at(b, at + 8)],
        digits: b[at + number::DIGITS],
        flags: b[at + number::FLAGS],
        fraction_digits: b[at + number::FRACTION_DIGITS],
    }))
}

/// The digits tag (`hud#`) the globals (`matg`) name in their first
/// interface element (the block `+0x140`, E `0x000e1a65`, `0x000e1a77`),
/// reference `+0xb0` (datum `+0xbc`, E `0x000e1a81`): its bytes.
fn digits_tag(map: &H1Map) -> Result<&[u8]> {
    let g = map
        .tags()
        .iter()
        .find(|t| t.group == h1map::Group::new(b"matg"))
        .ok_or_else(|| Error::Invalid("no matg".into()))?;
    let gb = map.tag_data(g)?;
    // Only the first element is read; its size is past `+0xc0`.
    let first = crate::h1::elements(map, gb, 0x140, 0xc0)?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("matg has no interface element".into()))?;
    let n = crate::h1::reference(map, first, 0xb0)
        .ok_or_else(|| Error::Invalid("matg names no hud digits".into()))?;
    let t = map.tag(n)?;
    if t.group != h1map::Group::new(b"hud#") {
        return Err(Error::Invalid(format!("matg +0xb0 is a {}", t.group)));
    }
    Ok(map.tag_data(t)?)
}

/// Halo 1's digit width, `hud#` byte `+0x11` (E `0x000e1d00`; V: 9 on
/// `ui\hud\counter`), the cell each digit of a number takes.
pub fn digit_width(map: &H1Map) -> Result<i16> {
    let w = *digits_tag(map)?.get(0x11).ok_or(Error::Truncated("hud#"))?;
    Ok(i16::from(w as i8))
}

/// The height of Halo 1's digit sprites: the digits bitmap (`hud#` `+0x00`)
/// sequence 0, its first sprite's height in pixels (V: 12 on
/// `ui\hud\bitmaps\combined\hud_counter_numbers`, every digit's sprite
/// the same height). Halo 1 draws each digit as its sprite at its own size
/// from the digit's point (E `0x000e1a60` through the element draw).
pub fn digit_height(map: &H1Map) -> Result<i16> {
    let b = digits_tag(map)?;
    let bm = crate::h1::reference(map, b, 0x00)
        .ok_or_else(|| Error::Invalid("hud# has no bitmap".into()))?;
    let bt = map.tag(bm)?;
    let bm = h1map::bitmap::Bitmap::read(map, bt)?;
    let (bounds, _) = sprite(&bm, 0).ok_or_else(|| Error::Invalid("hud# bitmap draws nothing".into()))?;
    let s = bm.sequences.first().ok_or_else(|| Error::Invalid("hud# bitmap has no sequence".into()))?;
    let image = s.sprites.first().map_or(s.first_image, |p| p.image);
    let img = bm
        .images
        .get(usize::try_from(image).unwrap_or(usize::MAX))
        .ok_or_else(|| Error::Invalid("hud# sprite image".into()))?;
    Ok(((bounds[3] - bounds[2]) * f32::from(img.height)).round() as i16)
}

/// The font a Halo 2 number text draws in: the text widget's byte `+0x30`
/// (`+0x31`, `+0x32` for half and quarter screen) picks it, 0 the engine's
/// font 6 and 1 its font 8 (E `0x22a85b`, called from the string fetch
/// `0x22a9b1`; the draw `0x22aa16` sets it through `0x13edb0`). Font 6 is
/// `conduit-13` (V: `maps\fonts\font_table.txt` line 6). The converter
/// writes 0, as retail `frag_count` and `plasma_count` hold.
pub const TEXT_FONT: u8 = 0;

/// The rows of `conduit-13`'s digits that have ink, from the text's top
/// (start inclusive, end exclusive). The draw (E `0x22aa16`) puts the
/// text's rectangle top at the widget's point (`+0x10` the y, the
/// measured height under it, `0x22ac83`); a glyph's bitmap top is at the
/// baseline less its y offset, which is the ascent on every glyph
/// (docs/formats/fonts.md), so its first row is the rectangle's top. The
/// digits' ink is rows 5 to 16 of their 21-row bitmaps (V: `0`..`9` of
/// `conduit-13`, ascent 16, descent 4; `hud_layout_disc` reads them).
pub const TEXT_DIGIT_INK: (i16, i16) = (5, 17);

/// Where a Halo 2 number text justified to end ([`JUSTIFY_RIGHT`]) is
/// placed to stand where Halo 1's number does: its x where Halo 1's last
/// digit ends, its y so the digits' ink ([`TEXT_DIGIT_INK`]) is centred on
/// Halo 1's digit sprites (`h` tall). As a Halo 2 offset from the same
/// anchor.
///
/// Across: Halo 1 draws the digits right to left, each a digit cell
/// (`hud#` digit width `w`) further left (E `0x000e1d00`..`0x000e1d18`),
/// from a start that the anchor moves (E the switch `0x000e1bd1` on the
/// anchor, table `0x000e2088`): on a left anchor right by `(cells - 2) * w`
/// (`0x000e1bd8`), where `cells` is the digits plus one with flag `0x04`;
/// on a right anchor not at all (`0x000e1c52`); centred by half of
/// `(cells - 1) * w` (`0x000e1c12`). A digit's sprite lies right of its
/// point on a left anchor, left of it on a right one and centred on it on
/// the centre (`0x000e0330`), so the number ends at the start plus `w`,
/// at the start, or at the start plus `w / 2`.
///
/// Down: Halo 1's digit sprite lies below its point on a top anchor, above
/// it on a bottom one and centred on the centre (`0x000e0330`). Halo 2's
/// text always lies below its point (E `0x22aa16`), its digits' ink rows
/// [`TEXT_DIGIT_INK`] under it. Putting that point at Halo 1's y drew the
/// ink 5 rows low, out of the bottom of the grenade indicator.
///
/// Fraction digits are not drawn by any `grhi` on the disc and are refused.
pub fn number_point(n: &Number, w: i16, h: i16) -> Result<(i16, i16)> {
    if n.fraction_digits != 0 {
        return Err(Error::Invalid("a number with fraction digits".into()));
    }
    let cells = i16::from(n.digits) + i16::from(n.flags & 0x04 != 0);
    let x = match n.anchor {
        Anchor::TopLeft | Anchor::BottomLeft => n.offset.0 + (cells - 2) * w + w,
        Anchor::TopRight | Anchor::BottomRight => n.offset.0,
        Anchor::Centre => n.offset.0 + (cells - 1) * w / 2 + w / 2,
    };
    let (x, y) = to_halo2_offset(n.anchor, (x, n.offset.1));
    // The top of Halo 1's digit sprite, in screen directions.
    let top = match n.anchor {
        Anchor::TopLeft | Anchor::TopRight => y,
        Anchor::BottomLeft | Anchor::BottomRight => y - h,
        Anchor::Centre => y - h / 2,
    };
    let (ink0, ink1) = TEXT_DIGIT_INK;
    // Centre the ink on the sprite: top + h/2 = y' + (ink0 + ink1)/2.
    Ok((x, top + (h - (ink0 + ink1)) / 2))
}

/// The text widget for grenade type `k`'s count: input `20 + k`, shown
/// with that type's other widgets ([`GRENADE_STATES`]), standing where
/// Halo 1's number does ([`number_point`]), in [`TEXT_FONT`].
pub fn grenade_count(n: &Number, k: usize, w: i16, h: i16, shader: TagId) -> Result<TextWidget> {
    let state = *GRENADE_STATES
        .get(k)
        .ok_or_else(|| Error::Invalid(format!("grenade type {k}")))?;
    Ok(TextWidget {
        name: format!("h1_grenade_count_{k}"),
        inputs: [INPUT_GRENADES + k as u8, 0, 0, 0],
        state,
        anchor: n.anchor,
        flags: TEXT_NUMBER,
        shader,
        string: TEXT_INPUT1,
        justification: JUSTIFY_RIGHT,
        offset: number_point(n, w, h)?,
    })
}

/// The Halo 2 text input that shows the value a Halo 1 weapon number's
/// state does. Halo 1's number values (E `0x000dbc4e`..`0x000dbc99`): state
/// 0 the magazine's `+6`, state 1 its `+8` (the per-magazine HUD block
/// `0x000be2f0`: `+0x12`, `+0x0e`), state 3 `(1 - age) * 100`.
/// Halo 2's (input switch `0x0013ad48`, its weapon block from `0x0013afa9`
/// and `0x00100520`): `0x30` the block's `+6`, the magazine's `+0x22c`, its
/// rounds loaded (the same field the magnum's ammo meter divides by the
/// magazine size, `0x36`); `0x33` the block's `+8`, `0x001008f0`'s count,
/// read from the magazine's `+0x22a`, the rounds in reserve (V: retail
/// Halo 2's ammo counts, the battle rifle's, magnum's, shotgun's and
/// rocket launcher's text, read it); `0x32` `100 - age * 100`
/// (`0x0013aee3`), the battery's percentage (V: retail Halo 2's plasma
/// rifle, energy sword and sentinel beam battery counts read it). Halo 1's magazine keeps its
/// reserve and loaded rounds at `+6` and `+8`, the order Halo 2's keeps
/// them in at `+0x22a` and `+0x22c` (inferred: the same field order).
pub fn weapon_number_input(s: WeaponState) -> Option<u8> {
    match s {
        WeaponState::TotalAmmo => Some(0x33),
        WeaponState::LoadedAmmo => Some(0x30),
        WeaponState::Battery => Some(0x32),
        _ => None,
    }
}

/// The text widget for weapon HUD number `e`, or why it has none: Halo 2
/// text on [`weapon_number_input`], standing where Halo 1's number does
/// ([`number_point`], as the grenade count), with the weapon's other
/// widgets' state flags ([`STATE`]).
///
/// The number's scale is not the element's: Halo 1's number draw takes
/// its scale from its eighth argument (E `0x000e1b3d`), which the weapon
/// HUD passes as 0 (`0x000dc0d6`), and a scale not above 0 is 1.0
/// (`0x000e1b58`). The disc's weapon numbers hold 0 there (V).
///
/// Refused: hidden or split-screen-only numbers (as the bitmap entries),
/// a colour of 0 ([`static_alpha`]), states no Halo 2 input shows (the
/// sniper rifle's zoom numbers, states 6 and 7), numbers shown in
/// magazines (`+0x78` bit 0: Halo 2's text cannot divide) and fraction
/// digits ([`number_point`]). Halo 1's leading zeros (flags bit 0: a
/// three-digit count of 60 shows `060`, E `0x000e1f5d`..`0x000e1f6f`) are
/// not drawn: no padding of Halo 2's number text is established.
pub fn weapon_number(
    e: &WeaponEntry,
    w: i16,
    h: i16,
    shader: TagId,
) -> std::result::Result<TextWidget, Skip> {
    let WeaponEntryKind::Number(state, n, magazines) = &e.kind else {
        return Err(Skip::NothingDrawn);
    };
    if e.hidden {
        return Err(Skip::Hidden);
    }
    if e.screen == SPLIT_SCREEN_ONLY {
        return Err(Skip::SplitScreen);
    }
    if e.colour == 0 {
        return Err(Skip::Invisible);
    }
    let input = weapon_number_input(*state).ok_or(Skip::NumberState(*state))?;
    if *magazines {
        return Err(Skip::Magazines);
    }
    if n.fraction_digits != 0 {
        return Err(Skip::Fraction);
    }
    let offset = number_point(n, w, h).map_err(|_| Skip::Fraction)?;
    Ok(TextWidget {
        name: "h1_weapon_number".to_owned(),
        inputs: [input, 0, 0, 0],
        state: STATE,
        anchor: n.anchor,
        flags: TEXT_NUMBER,
        shader,
        string: TEXT_INPUT1,
        justification: JUSTIFY_RIGHT,
        offset,
    })
}

/// The `nhdt` root drawing `widgets` and `texts` and nothing else: the
/// root's reference null with its group word as on every retail `nhdt`, no
/// effects.
pub fn nhdt(widgets: &[Widget], texts: &[TextWidget]) -> Block {
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
    if !texts.is_empty() {
        let mut block = Block::new(Vec::with_capacity(texts.len() * text::SIZE));
        for t in texts {
            let at = block.bytes.len() as u32;
            block.bytes.extend(t.bytes());
            block.string_id(at + text::NAME as u32, &t.name);
            block.tag_ref(at + text::SHADER as u32, t.shader);
            block.string_id(at + text::STRING as u32, t.string);
        }
        root.block(NHDT_TEXT_WIDGETS as u32, texts.len() as u32, block);
    }
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
            colour: 0xdc2896ff,
        }
    }

    #[test]
    fn a_static_of_colour_zero_is_not_drawn() {
        assert_eq!(static_alpha(0), None);
        assert_eq!(static_alpha(0xdc2896ff), Some(0xdc));
        assert_eq!(static_alpha(0xffaad5ff), Some(0xff));
        assert_eq!(static_alpha(0x002896ff), Some(0xff), "additive in Halo 1: shown");
        let mut s = entry(WeaponEntryKind::Static(WeaponState::LoadedAmmo));
        s.colour = 0;
        assert_eq!(weapon_widget(&s, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::Invisible));
        // Meters and crosshairs keep their own colour rules.
        s.kind = WeaponEntryKind::Meter(WeaponState::LoadedAmmo, [0, 60, 1]);
        assert!(weapon_widget(&s, Some(TagId(1)), TagId(2), TagId(3)).is_ok());
    }

    fn number_entry(state: WeaponState, magazines: bool) -> WeaponEntry {
        // The disc's full-screen count (V: bloodgulch's assault rifle).
        let n = Number {
            anchor: Anchor::TopLeft,
            offset: (29, 3),
            scale: [0.0, 0.0],
            digits: 3,
            flags: 1,
            fraction_digits: 0,
        };
        WeaponEntry {
            kind: WeaponEntryKind::Number(state, n, magazines),
            screen: 1,
            offset: n.offset,
            scale: n.scale,
            bitmap: None,
            sequence: 0,
            colour: 0xff2896ff,
            ..entry(WeaponEntryKind::Static(state))
        }
    }

    #[test]
    fn weapon_numbers_are_number_texts_on_halo2s_ammo_inputs() {
        let e = number_entry(WeaponState::TotalAmmo, false);
        let t = weapon_number(&e, 9, 12, TagId(7)).unwrap();
        assert_eq!(t.inputs, [0x33, 0, 0, 0]);
        assert_eq!((t.flags, t.string, t.justification), (TEXT_NUMBER, TEXT_INPUT1, JUSTIFY_RIGHT));
        assert_eq!((t.anchor, t.state, t.shader), (Anchor::TopLeft, STATE, TagId(7)));
        // Three cells from 29: the last ends at 29 + 2 * 9; the ink centred
        // on the digit sprite's rows 3..15.
        assert_eq!(t.offset, (47, -2));
        let input = |s| weapon_number(&number_entry(s, false), 9, 12, TagId(7)).map(|t| t.inputs[0]);
        assert_eq!(input(WeaponState::LoadedAmmo), Ok(0x30));
        assert_eq!(input(WeaponState::Battery), Ok(0x32));
        assert_eq!(input(WeaponState::Other(6)), Err(Skip::NumberState(WeaponState::Other(6))));
        assert_eq!(input(WeaponState::Heat), Err(Skip::NumberState(WeaponState::Heat)));
        let mags = number_entry(WeaponState::TotalAmmo, true);
        assert_eq!(weapon_number(&mags, 9, 12, TagId(7)), Err(Skip::Magazines));
        let mut e = number_entry(WeaponState::TotalAmmo, false);
        e.screen = SPLIT_SCREEN_ONLY;
        assert_eq!(weapon_number(&e, 9, 12, TagId(7)), Err(Skip::SplitScreen));
        e.screen = 0;
        e.colour = 0;
        assert_eq!(weapon_number(&e, 9, 12, TagId(7)), Err(Skip::Invisible));
        e.colour = 1;
        if let WeaponEntryKind::Number(_, n, _) = &mut e.kind {
            n.fraction_digits = 3;
        }
        assert_eq!(weapon_number(&e, 9, 12, TagId(7)), Err(Skip::Fraction));
        // Not a bitmap widget.
        let e = number_entry(WeaponState::TotalAmmo, false);
        assert_eq!(weapon_widget(&e, Some(TagId(1)), TagId(2), TagId(3)), Err(Skip::Text));
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
    fn a_format_without_an_alpha_byte_is_rewritten_as_linear_argb() {
        use h1map::bitmap::{self as b, Image, ImageKind, PixelFormat as F};
        // 2x2 swizzled X8R8G8B8 (a 2x2 swizzle is row order), B,G,R,X.
        let img = Image {
            width: 2,
            height: 2,
            depth: 1,
            kind: ImageKind::D2,
            format: F::X8R8G8B8,
            flags: b::FLAG_SWIZZLED | b::FLAG_POWER_OF_TWO,
            registration: [1, 1],
            mip_count: 1,
            pixel_offset: 0,
            pixel_size: 20,
            raw: [0; b::IMAGE_LEN],
        };
        let px = [1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0, 13, 14, 15, 0];
        let (argb, out) = alpha_scaled_argb(&img, &px, 0x28).unwrap();
        assert_eq!(argb.format, F::A8R8G8B8);
        assert_eq!(argb.flags, b::FLAG_LINEAR | b::FLAG_POWER_OF_TWO);
        assert_eq!((argb.mip_count, argb.registration), (0, [1, 1]));
        // Rows padded to 64 bytes.
        assert_eq!(&out[..8], &[1, 2, 3, 0x28, 4, 5, 6, 0x28]);
        assert_eq!(&out[64..72], &[7, 8, 9, 0x28, 10, 11, 12, 0x28]);
        let back = h1bitmaps::decode_image(&argb, &out).unwrap();
        assert_eq!(&back[0].rgba[..8], &[3, 2, 1, 0x28, 6, 5, 4, 0x28]);
        assert_eq!(&back[0].rgba[8..], &[9, 8, 7, 0x28, 12, 11, 10, 0x28]);
    }

    fn number(anchor: Anchor, digits: u8, flags: u8) -> Number {
        Number { anchor, offset: (107, 3), scale: [1.0, 1.0], digits, flags, fraction_digits: 0 }
    }

    #[test]
    fn halo1_numbers_end_where_their_last_digit_does() {
        // The disc's grenade count: one digit, top left, (107, 3).
        // Across: the last digit's end. Down: conduit-13's digit ink (rows
        // 5..17 under the point) centred on Halo 1's 12-row digit sprites.
        assert_eq!(number_point(&number(Anchor::TopLeft, 1, 1), 9, 12).unwrap(), (107, -2));
        assert_eq!(number_point(&number(Anchor::TopLeft, 3, 0), 9, 12).unwrap(), (125, -2));
        assert_eq!(number_point(&number(Anchor::TopLeft, 2, 4), 9, 12).unwrap(), (125, -2));
        assert_eq!(number_point(&number(Anchor::BottomRight, 3, 0), 9, 12).unwrap(), (-107, -20));
        assert_eq!(number_point(&number(Anchor::Centre, 3, 0), 10, 12).unwrap(), (122, -8));
        let mut n = number(Anchor::TopLeft, 1, 0);
        n.fraction_digits = 1;
        assert!(number_point(&n, 9, 12).is_err());
    }

    #[test]
    fn a_grenade_count_is_a_number_text_on_its_type_input() {
        let t = grenade_count(&number(Anchor::TopLeft, 1, 1), 1, 9, 12, TagId(7)).unwrap();
        assert_eq!(t.inputs, [21, 0, 0, 0]);
        assert_eq!(t.state, GRENADE_STATES[1]);
        let b = t.bytes();
        assert_eq!(b.len(), 0x54);
        assert_eq!(&b[text::INPUTS..text::INPUTS + 4], &[21, 0, 0, 0]);
        assert_eq!(&b[text::ANCHOR..text::ANCHOR + 4], &[Anchor::TopLeft as u8, 0, 1, 0]);
        assert_eq!(&b[text::JUSTIFICATION..text::JUSTIFICATION + 2], &[2, 0]);
        for at in text::OFFSETS {
            assert_eq!(&b[at..at + 4], &[107, 0, 0xfe, 0xff], "y -2: the ink on Halo 1's digits");
        }
        assert_eq!(&b[text::FONTS..text::FONTS + 4], &[TEXT_FONT, TEXT_FONT, TEXT_FONT, 0]);
        assert!(grenade_count(&number(Anchor::TopLeft, 1, 1), 2, 9, 12, TagId(7)).is_err());
        let root = nhdt(&[], &[t]);
        let child = root
            .fixups
            .iter()
            .find_map(|f| match &f.kind {
                FixupKind::Block { child, .. } if f.offset == NHDT_TEXT_WIDGETS as u32 => Some(child),
                _ => None,
            })
            .expect("text block");
        assert_eq!(child.bytes.len(), text::SIZE);
        let kinds: Vec<_> = child.fixups.iter().map(|f| (f.offset, f.kind.clone())).collect();
        assert!(kinds.contains(&(0x20, FixupKind::TagRef(TagId(7)))));
        assert!(kinds.contains(&(0x28, FixupKind::StringId(TEXT_INPUT1.into()))));
        assert!(kinds.contains(&(0x00, FixupKind::StringId("h1_grenade_count_1".into()))));
    }

    #[test]
    fn the_motion_sensor_centre_is_halo1s_placement_up_from_the_bottom_left() {
        let mut b = vec![0u8; unhi::SIZE];
        let at = unhi::MOTION_SENSOR;
        b[at..at + 2].copy_from_slice(&45i16.to_le_bytes());
        b[at + 2..at + 4].copy_from_slice(&43i16.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&1.0f32.to_le_bytes());
        b[at + 8..at + 12].copy_from_slice(&1.0f32.to_le_bytes());
        assert_eq!(sensor_centre(&b).unwrap(), [45, -43]);
        b[at + 4..at + 8].copy_from_slice(&2.0f32.to_le_bytes());
        assert!(sensor_centre(&b).is_err());
        assert!(sensor_centre(&b[..at]).is_err());
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
    fn halo2_places_each_anchor_as_halo1_does() {
        for anchor in [
            Anchor::TopLeft,
            Anchor::TopRight,
            Anchor::BottomLeft,
            Anchor::BottomRight,
            Anchor::Centre,
        ] {
            let mut e = element(Role::Static);
            e.anchor = anchor;
            e.offset = to_halo2_offset(anchor, (36, 17));
            let w = widget_for(&e, TagId(1), registration(anchor), TagId(2), TagId(3));
            assert_eq!(halo2_rect(&w, (88, 14)), halo1_rect(anchor, (36, 17), (88, 14)), "{anchor:?}");
        }
        // The cyborg's health meter: 36 in from the right, 17 down.
        assert_eq!(halo1_rect(Anchor::TopRight, (36, 17), (88, 14)), [-124, 17, -36, 31]);
    }

    #[test]
    fn clear_meter_texels_lose_their_colour_and_stay_clear() {
        use h1map::bitmap::PixelFormat as F;
        // A8Y8: Y then A. A clear texel with luminance shows in Halo 2's
        // filled part; once black it does not.
        let mut px = vec![200, 0, 90, 255, 30, 0x40];
        assert_eq!(legacy_meter_alpha([200, 200, 200, 0], true), 200);
        assert_eq!(legacy_meter_alpha([200, 200, 200, 0], false), 0);
        clear_meter_colour(F::A8Y8, &mut px).unwrap();
        assert_eq!(px, [0, 0, 90, 255, 30, 0x40], "visible texels unchanged");
        assert_eq!(legacy_meter_alpha([0, 0, 0, 0], true), 0);
        let mut px = vec![1, 2, 3, 0, 4, 5, 6, 7];
        clear_meter_colour(F::A8R8G8B8, &mut px).unwrap();
        assert_eq!(px, [0, 0, 0, 0, 4, 5, 6, 7]);
        assert!(clear_meter_colour(F::Dxt5, &mut [0; 16]).is_none());
        assert!(clear_meter_colour(F::Ay8, &mut [0; 4]).is_some());
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
        let root = nhdt(&ws, &[]);
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
