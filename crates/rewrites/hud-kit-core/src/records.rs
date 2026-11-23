//! Elements to `nhdt` widget records (docs/game/hud-kit.md, "The model"):
//! a `0x64`-byte bitmap widget per meter, icon and pointer, a `0x54`-byte
//! text widget per text and gauge, written at the field offsets of the game's
//! widget draw ([`bitmap`], [`text`]; `crates/formats/tags/src/defs/nhdt.txt`
//! and `hud_edit::layout` name the same fields, and `hud-kit`'s tests read
//! these bytes back through the HUD editor's model), with the
//! [`Templates`]' datums written in. The same bytes go to the game and to
//! the preview.
//!
//! What the bytes leave to the runtime's hooks: the four show words are
//! zero, so the retail gate (E `0x0013ac87`) never draws a kit widget by
//! itself; the inputs are [`KIT_INPUT`] (the hook answers `hud_input1`
//! with [`Widget::input`]); a text's string is the template's (the hook
//! answers the text, a gauge's the character for its fill:
//! [`Widget::text`]). A text hidden in a screen mode has the font byte
//! [`NO_FONT`] there, so the game's own draw skips it (E `0x0022a85b`).

use map::{FourCC, TagId};

use crate::Mode;
use crate::model::{Element, GAUGE_FONT, Glyphs, Hud, Justify, Kind, Origin, Place, Value};
use crate::templates::{BITM, Templates};

/// A widget block's kind: the root's two blocks (E `0x0013a532`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WidgetKind {
    Bitmap,
    Text,
}

impl WidgetKind {
    pub fn size(self) -> usize {
        match self {
            WidgetKind::Bitmap => BITMAP_SIZE,
            WidgetKind::Text => TEXT_SIZE,
        }
    }
    /// Where the root holds this kind's block (count, address).
    pub fn root_field(self) -> u32 {
        match self {
            WidgetKind::Bitmap => ROOT_BITMAPS,
            WidgetKind::Text => ROOT_TEXTS,
        }
    }
}

/// The `nhdt` root's size, and where its two widget blocks are (count,
/// then address: E `0x0013a532`).
pub const ROOT_SIZE: usize = 0x28;
pub const ROOT_BITMAPS: u32 = 0x08;
pub const ROOT_TEXTS: u32 = 0x10;
pub const BITMAP_SIZE: usize = 0x64;
pub const TEXT_SIZE: usize = 0x54;

/// Bitmap widget fields (E: the gate `0x0013ac87`, the inputs
/// `0x0013ad48`, the draw `0x0022aff1`).
pub mod bitmap {
    pub const NAME: usize = 0x00;
    pub const INPUTS: usize = 0x04;
    /// The gate's 20 state bytes: four show words, four hide words, three
    /// thresholds and a pad.
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    pub const ANCHOR: usize = 0x1c;
    pub const FLAGS: usize = 0x1e;
    /// Tag references: the group's four bytes, then the datum.
    pub const BITMAP: usize = 0x20;
    pub const SHADER: usize = 0x28;
    /// `i8` per screen mode.
    pub const SEQUENCES: usize = 0x30;
    /// Two `i16` per screen mode.
    pub const OFFSETS: [usize; 3] = [0x34, 0x38, 0x3c];
    /// Two `f32` per screen mode.
    pub const REGISTRATIONS: [usize; 3] = [0x40, 0x48, 0x50];
    /// The effects block (count, address).
    pub const EFFECTS: usize = 0x58;
    pub const SPECIAL: usize = 0x60;
}

/// Text widget fields (E: the draw `0x0022aa16`, its string `0x0022a871`).
pub mod text {
    pub const NAME: usize = 0x00;
    pub const INPUTS: usize = 0x04;
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    pub const ANCHOR: usize = 0x1c;
    pub const FLAGS: usize = 0x1e;
    pub const SHADER: usize = 0x20;
    /// The string ID word.
    pub const STRING: usize = 0x28;
    pub const JUSTIFICATION: usize = 0x2c;
    /// `i8` per screen mode.
    pub const FONTS: usize = 0x30;
    pub const OFFSETS: [usize; 3] = [0x40, 0x44, 0x48];
    pub const EFFECTS: usize = 0x4c;
}

/// The input byte of every kit widget: input 0, "zero" (E `0x0013ad48`).
pub const KIT_INPUT: u8 = 0;
/// The font byte of a text in a mode it is hidden in: no font, no text.
pub const NO_FONT: i8 = -1;

/// One compiled widget.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: &'static str,
    pub kind: WidgetKind,
    pub bytes: Vec<u8>,
    /// The part of its sprite a bitmap widget draws
    /// ([`crate::templates::ArtTemplate::crop`]): not in the record; the
    /// runtime narrows the sprite's bounds to it.
    pub crop: Option<[f32; 4]>,
    /// A cropped meter's `hud_input1` at fill 0 and 1
    /// ([`crate::templates::ArtTemplate::ramp`]).
    pub ramp: Option<[f32; 2]>,
    /// A gauge's pictures ([`Kind::Gauge`]): not in the record; the
    /// string hook gives the character for the fill ([`Widget::text`]).
    pub glyphs: Option<Glyphs>,
}

impl Widget {
    /// The `hud_input1` the input hook gives for `fill` (0 to 1): the fill
    /// itself, or for a cropped meter the blue across its crop, `b0 + fill
    /// × (b1 − b0)`.
    pub fn input(&self, fill: f32) -> f32 {
        let fill = if fill.is_nan() {
            0.0
        } else {
            fill.clamp(0.0, 1.0)
        };
        match self.ramp {
            Some([b0, b1]) => b0 + fill * (b1 - b0),
            None => fill,
        }
    }

    /// The string the string hook gives a text widget at `v`: the value's
    /// text, or for a gauge the character of its picture for the value's
    /// fill ([`Glyphs::text`]).
    pub fn text(&self, v: &Value) -> String {
        match self.glyphs {
            Some(g) => g.text(v.fill),
            None => v.text.clone(),
        }
    }
}

/// A HUD compiled: its widgets in the game's draw order (bitmaps, then
/// texts, each in element order: E `0x0013a532`), and its channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub hud: &'static str,
    pub widgets: Vec<Widget>,
    pub channels: Vec<&'static str>,
}

impl Compiled {
    pub fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }
    pub fn of(&self, kind: WidgetKind) -> impl Iterator<Item = &Widget> {
        self.widgets.iter().filter(move |w| w.kind == kind)
    }
    /// The block of `kind`'s records, end to end, as the game reads one.
    pub fn block(&self, kind: WidgetKind) -> Vec<u8> {
        self.of(kind)
            .flat_map(|w| w.bytes.iter().copied())
            .collect()
    }
}

/// A compile failure: the element and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<T>(why: impl Into<String>) -> Result<T, Error> {
    Err(Error(why.into()))
}

/// The anchor and the offset per mode `at` gives on `t` (a hidden mode
/// keeps the fullscreen offset; the gate hides it). An offset from the
/// grenade row is added to the row's point, in the row's anchor. A place
/// from the row in some modes and from its own anchor in the others
/// (`Place::split_origin`) has the row's anchor, the one a widget has;
/// another is an error.
pub fn placed(at: &Place, t: &Templates) -> Result<(u16, [(i16, i16); 3]), Error> {
    let own = Mode::ALL.map(|m| at.offset(m).unwrap_or(at.full));
    if Mode::ALL.iter().all(|m| at.origin_in(*m) == Origin::Anchor) {
        return Ok((at.anchor as u16, own));
    }
    let row = match t.row.as_ref() {
        Ok(r) => r,
        Err(why) => return err(format!("the grenade row: {why}")),
    };
    let mut anchor = None;
    let mut mixed = false;
    let mut out = own;
    for (i, m) in Mode::ALL.iter().enumerate() {
        match row.at(at.origin_in(*m), *m) {
            Some((a, (x, y))) => {
                anchor = Some(a);
                out[i] = (x + own[i].0, y + own[i].1);
            }
            None => mixed = true,
        }
    }
    let anchor = anchor.expect("a row origin in some mode");
    if mixed && anchor != at.anchor as u16 {
        return err(format!(
            "a place from anchor {} in some modes and the grenade row (anchor {anchor}) in others: a widget has one anchor",
            at.anchor as u16
        ));
    }
    Ok((anchor, out))
}

fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A tag reference: the group as the map stores it, then the datum.
fn put_tag(b: &mut [u8], at: usize, group: FourCC, id: TagId) {
    put_u32(b, at, group.as_u32());
    put_u32(b, at + 4, id.0);
}

/// The fields both kinds share: the four inputs (the state bytes stay
/// zero), the anchor, the offsets.
fn common(
    b: &mut [u8],
    inputs: usize,
    anchor_at: usize,
    offsets: [usize; 3],
    at: &Place,
    t: &Templates,
) -> Result<(), Error> {
    b[inputs..inputs + 4].fill(KIT_INPUT);
    let (anchor, placed) = placed(at, t)?;
    put_u16(b, anchor_at, anchor);
    for (o, (x, y)) in offsets.iter().zip(placed) {
        put_u16(b, *o, x as u16);
        put_u16(b, o + 2, y as u16);
    }
    Ok(())
}

fn bitmap_record(e: &Element, t: &Templates) -> Result<Widget, Error> {
    use bitmap as f;
    let art = e.kind.art().expect("a bitmap element has art");
    let a = match t.art(&art) {
        Ok(a) => a,
        Err(why) => return err(format!("{}: {why}", e.id)),
    };
    let mut b = vec![0u8; BITMAP_SIZE];
    common(&mut b, f::INPUTS, f::ANCHOR, f::OFFSETS, &e.at, t)?;
    put_tag(&mut b, f::BITMAP, BITM, a.bitmap);
    put_tag(&mut b, f::SHADER, a.shader.0, a.shader.1);
    for (i, m) in Mode::ALL.iter().enumerate() {
        b[f::SEQUENCES + i] = a.sequences[i] as u8;
        let r = f::REGISTRATIONS[m.index()];
        b[r..r + 4].copy_from_slice(&e.at.registration[0].to_le_bytes());
        b[r + 4..r + 8].copy_from_slice(&e.at.registration[1].to_le_bytes());
    }
    Ok(Widget {
        id: e.id,
        kind: WidgetKind::Bitmap,
        bytes: b,
        crop: a.crop,
        ramp: matches!(e.kind, Kind::Meter { .. })
            .then_some(a.ramp)
            .flatten(),
        glyphs: None,
    })
}

fn text_record(e: &Element, t: &Templates) -> Result<Widget, Error> {
    use text as f;
    let (font, justify, glyphs) = match &e.kind {
        Kind::Text { font, justify, .. } => (*font, *justify, None),
        // A gauge's picture has its top-left corner at the widget's point.
        Kind::Gauge { glyphs } => (GAUGE_FONT, Justify::Start, Some(*glyphs)),
        _ => unreachable!("a text element"),
    };
    let tt = match t.text.as_ref() {
        Ok(tt) => tt,
        Err(why) => return err(format!("{}: {why}", e.id)),
    };
    let mut b = vec![0u8; TEXT_SIZE];
    common(&mut b, f::INPUTS, f::ANCHOR, f::OFFSETS, &e.at, t)?;
    put_tag(&mut b, f::SHADER, tt.shader.0, tt.shader.1);
    put_u32(&mut b, f::STRING, tt.string);
    put_u16(&mut b, f::JUSTIFICATION, justify as u16);
    for m in Mode::ALL {
        let byte = if e.at.offset(m).is_some() {
            font.byte()
        } else {
            NO_FONT
        };
        b[f::FONTS + m.index()] = byte as u8;
    }
    Ok(Widget {
        id: e.id,
        kind: WidgetKind::Text,
        bytes: b,
        crop: None,
        ramp: None,
        glyphs,
    })
}

/// Compiles `hud` against `t`.
pub fn compile(hud: &Hud, t: &Templates) -> Result<Compiled, Error> {
    if let Some(p) = hud.problems().first() {
        return err(format!("{}: {p}", hud.name));
    }
    let mut bitmaps = Vec::new();
    let mut texts = Vec::new();
    let mut channels = Vec::new();
    for e in &hud.elements {
        match &e.kind {
            Kind::Meter { .. } | Kind::Icon { .. } | Kind::Pointer { .. } => {
                bitmaps.push(bitmap_record(e, t)?)
            }
            Kind::Text { .. } | Kind::Gauge { .. } => texts.push(text_record(e, t)?),
            _ => channels.push(e.id),
        }
    }
    bitmaps.extend(texts);
    Ok(Compiled {
        hud: hud.name,
        widgets: bitmaps,
        channels,
    })
}
