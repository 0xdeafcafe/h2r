//! The engine's HUD walk over one definition (`nhdt`), computed from the
//! definition's bytes and the HUD state the game keeps for a player
//! (docs/graphics/native-hud-overlay.md, "The walk in Rust").
//!
//! (Code, `h2 disasm`.) The walk `0x0013a532` asks `0x0013a050` for four
//! state words and the weapon record, then goes through the definition's
//! screen-effect widgets (`+0x20`, `0x50` bytes each), bitmap widgets
//! (`+0x08`, `0x64`) and text widgets (`+0x10`, `0x54`), in that order.
//! Each widget passes the gate `0x0013ac87`, takes its four inputs from
//! `0x0013ad48` and is drawn by `0x0022b8e2`, `0x0022aff1` or
//! `0x0022aa16`. Every function here is one of those, in the original's
//! operation order, so a value here has the bits the guest's has:
//!
//! | Here | Guest |
//! |---|---|
//! | [`state_words`] | `0x0013a050` |
//! | [`gate`] | `0x0013ac87` |
//! | [`inputs`] | `0x0013ad48` |
//! | [`mode`] | `0x0013a690` |
//! | [`anchor_point`] | `0x001396c7` (corners and the centre) |
//! | [`sprite`] | `0x0022ae21` |
//! | [`bitmap_widget`] | `0x0022aff1` |
//! | [`vertices`] | `0x0022a664`, up to its state setup |
//! | [`screen_widget`] | `0x0022b8e2` |
//! | [`text_widget`] | `0x0022aa16`, up to its string |
//! | [`shown`] | `0x0013a532` |
//! | [`definitions`] | `0x001394b9` |
//!
//! Guest-free: the caller reads the record, the definition and the
//! bitmap tag (`scene_extract::hud`) and applies the writes a draw lists.

use crate::Mode;

/// Widget sizes in a definition's blocks.
pub const SCREEN_SIZE: usize = 0x50;
pub const BITMAP_SIZE: usize = 0x64;
pub const TEXT_SIZE: usize = 0x54;
/// The bytes of the HUD state record the walk reads.
pub const RECORD_SIZE: usize = 0x280;

/// The globals a widget draw writes: the four shader inputs
/// (`hud_input1`..`4`), three colours, and the bitmap and image the
/// shader quad binds.
pub mod global {
    pub const BITMAP: u32 = 0x004e_6978;
    pub const IMAGE: u32 = 0x004e_697c;
    pub const INPUTS: u32 = 0x004e_6980;
    pub const COLOUR_A: u32 = 0x004e_6990;
    pub const COLOUR_B: u32 = 0x004e_699c;
    pub const COLOUR_C: u32 = 0x004e_69a8;
}

/// Why the walk does not give a widget's draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Bytes the walk needs did not read.
    Read,
    /// Special types 1 and 2 set a player's emblem up (`0x001392c5`).
    Emblem,
    /// An anchor past the centre is a projected point.
    Anchor,
    /// The widget's effects read an input the callback computes by a call.
    Effect,
}

impl Refusal {
    /// The refusal's part of a counter's name.
    pub fn cause(self) -> &'static str {
        match self {
            Refusal::Read => "read",
            Refusal::Emblem => "emblem",
            Refusal::Anchor => "anchor",
            Refusal::Effect => "effect",
        }
    }
}

fn u8_at(b: &[u8], o: usize) -> u8 {
    b[o]
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    u16_at(b, o) as i16
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

/// What the walk reads outside the record and the definition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Session {
    /// `0x0022acb4` for the player: the user's unit record byte `+0x88`
    /// is 1 or 3.
    pub user_state: bool,
    /// The game engine's kind (`[[0x004e6948] + 8]`).
    pub engine: u32,
    /// The flash clock's count and scale (`[0x00510c54] + 8`, `+ 4`).
    pub clock: (i32, f32),
    /// `0x001392a9` for the player: `[0x004e69c0 + 4 * player]`, or 1.
    pub player_value: f32,
    /// The player's UI record value (`[0x00510c4c] + player * 0x6c + 0x28`).
    pub user_value: f32,
}

/// The four state words and the weapon record `0x0013a050` gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    /// The weapon record's offset in the HUD record, by the walk's slot.
    pub weapon: Option<usize>,
    pub a: u16,
    pub b: u16,
    pub c: u16,
    pub d: u16,
}

fn bit(on: bool, mask: u16) -> u16 {
    if on { mask } else { 0 }
}

/// `0x0013a050`: the walk's state words for `slot` (the walk's third
/// argument: 1 to 4 pick a weapon record) from the HUD `record`.
pub fn state_words(record: &[u8], slot: u32, session: &Session) -> Option<Words> {
    if record.len() < RECORD_SIZE {
        return None;
    }
    let weapon = match slot {
        1 => Some(0x88),
        2 => Some(0xc0),
        3 => Some(0xf8),
        4 => Some(0x50),
        _ => None,
    };
    let flag = |o: usize| u8_at(record, o) != 0;
    let grenade = i16_at(record, 0x36);
    let zoom = i16_at(record, 0x3c);
    let a = 1
        | bit(grenade == -1, 0x2)
        | bit(grenade == 0, 0x4)
        | bit(grenade == 1, 0x8)
        | bit(flag(0x30), 0x10)
        | bit(flag(0x31), 0x20)
        | bit(zoom == -1, 0x40)
        | bit(zoom == 0, 0x80)
        | bit(zoom == 1, 0x100)
        | bit(flag(0x32), 0x200)
        | bit(flag(0x3e), 0x400)
        | bit(flag(0x3f), 0x800)
        | bit(flag(0x40), 0x1000)
        | bit(session.user_state, 0x2000);
    let unit = u8_at(record, 0x4c);
    let b = bit(unit & 1 != 0, 0x1)
        | bit(unit & 8 != 0, 0x2)
        | bit(unit & 0x10 != 0, 0x4)
        | bit(unit & 4 != 0, 0x8)
        | bit(unit & 2 != 0, 0x10);
    let mut c = bit(slot == 1, 0x1) | bit(slot == 2, 0x2) | bit(slot == 3, 0x4);
    if let Some(w) = weapon {
        let word = |o: usize| i16_at(record, w + o);
        c |= bit(u8_at(record, w + 0x20) != 0, 0x40);
        if word(0x0a) != 0 && word(0x06) == 0 && word(0x0c) != 0 && word(0x08) == 0 {
            c |= 0x80;
        }
        // `comiss; jb` skips: an unordered compare skips too.
        if f32_at(record, w + 0x10) >= 1.0 {
            c |= 0x80;
        }
        c |= bit(u8_at(record, w + 0x24) != 0, 0x100)
            | bit(u8_at(record, w + 0x25) != 0, 0x200)
            | bit(u8_at(record, w + 0x26) != 0, 0x400);
    }
    let team = session.engine == 2;
    let d = bit(u8_at(record, 0x131) == 0, 0x1)
        | bit(u8_at(record, 0x131) == 1, 0x2)
        | bit(team && !flag(0x132), 0x4)
        | bit(team && flag(0x132), 0x8)
        | bit(flag(0x133), 0x10)
        | bit(!flag(0x133), 0x20)
        | bit(flag(0x174), 0x40)
        | bit(!flag(0x174), 0x80)
        | bit(flag(0x1d0), 0x100)
        | bit(!flag(0x1d0), 0x200)
        | bit(flag(0x21c), 0x400)
        | bit(flag(0x224), 0x800);
    Some(Words { weapon, a, b, c, d })
}

/// `0x0013ac87` on a widget's 20 state bytes (`+0x08`): shown when no
/// hide word (the second four) meets its state word and a show word (the
/// first four) does. With a weapon record, word C's bits `0x08`, `0x10`
/// and `0x20` come from the widget's threshold bytes (`+0x10`..`+0x12` of
/// the state) against the record's `+0x10`, `+0x06` and `+0x08`.
pub fn gate(state: &[u8], words: &Words, record: &[u8]) -> bool {
    let mut c = words.c;
    if let Some(w) = words.weapon {
        let mut set = |mask: u16, on: bool| c = if on { c | mask } else { c & !mask };
        // `comiss; jbe` clears: an unordered compare clears too.
        set(0x08, f32::from(state[0x10]) > f32_at(record, w + 0x10));
        set(0x10, i16_at(record, w + 6) < i16::from(state[0x11]));
        set(0x20, i16_at(record, w + 8) < i16::from(state[0x12]));
    }
    let given = [words.a, words.b, c, words.d];
    let any = |base: usize| (0..4).any(|i| u16_at(state, base + 2 * i) & given[i] != 0);
    any(0) && !any(8)
}

/// `0x0013ad48`: a widget's four inputs from its four input bytes.
pub fn inputs(bytes: &[u8], record: &[u8], weapon: Option<usize>, session: &Session) -> [f32; 4] {
    let clock = || session.clock.0 as f32 * session.clock.1;
    let with = |f: &dyn Fn(usize) -> f32| weapon.map_or(0.0, f);
    std::array::from_fn(|i| match bytes[i] {
        1 => 1.0,
        2 => clock(),
        3 => session.player_value,
        0x10 => f32_at(record, 0x2c),
        0x11 => f32_at(record, 0x28),
        // `comiss; jb` gives 0: an unordered compare does too.
        0x12 => {
            if f32_at(record, 0x20) >= 1.0 { 1.0 } else { 0.0 }
        }
        0x13 => {
            if u16_at(record, 0x34) == 0 { 1.0 } else { 0.0 }
        }
        0x14 => f32::from(i16_at(record, 0x38)),
        0x15 => f32::from(i16_at(record, 0x3a)),
        0x16 => {
            if 0.2 > f32_at(record, 0x2c) { clock() } else { 0.0 }
        }
        0x18 => 1.0 - f32_at(record, 0x44),
        0x30 => with(&|w| f32::from(i16_at(record, w + 6))),
        0x31 => with(&|w| f32_at(record, w + 0x14)),
        0x32 => with(&|w| 100.0 - f32_at(record, w + 0x10) * 100.0),
        0x33 => with(&|w| f32::from(i16_at(record, w + 8))),
        0x34 => with(&|w| f32_at(record, w + 0x18)),
        0x35 => with(&|w| if u8_at(record, w + 0x20) != 0 { 1.0 } else { 0.0 }),
        0x36 => with(&|w| match i16_at(record, w + 0x0a) {
            0 => 0.0,
            size => f32::from(i16_at(record, w + 6)) / f32::from(size),
        }),
        0x37 => with(&|w| if u8_at(record, w + 0x20) != 0 { clock() } else { 0.0 }),
        0x38 => with(&|w| 1.0 - f32_at(record, w + 0x10)),
        0x39 => with(&|w| f32_at(record, w + 0x28)),
        0x40 => f32_at(record, 0x1cc),
        0x41 => f32_at(record, 0x218),
        0x42 => session.user_value,
        0x43 => f32_at(record, 0x220),
        _ => 0.0,
    })
}

/// The view the HUD is laid out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    /// The render window (`0x004b9dd0`): top, left, bottom, right.
    pub window: [i16; 4],
    /// The HUD frame (`0x004b9dd8`): top, left, bottom, right.
    pub frame: [i16; 4],
    /// `0x00016a90`: a half-height view counts as a quarter for the
    /// bottom-right anchor.
    pub quarter: bool,
}

/// `0x0013a690`: the screen mode a widget anchored at `anchor` draws in.
pub fn mode(anchor: u16, screen: &Screen) -> Mode {
    let [top, left, bottom, right] = screen.window;
    let (w, h) = (right.wrapping_sub(left), bottom.wrapping_sub(top));
    if w >= 0x280 && h >= 0x1e0 {
        return Mode::Full;
    }
    if w < 0x280 && h < 0x1e0 {
        return Mode::Quarter;
    }
    if screen.quarter && anchor == 3 { Mode::Quarter } else { Mode::Half }
}

/// The CRT's `floor` and `ceil` on a float (`0x0002b460`, `0x0001d6d0`:
/// widened to a double, rounded under the CRT's own control word, stored
/// back as a float).
fn floor(v: f32) -> f32 {
    f64::from(v).floor() as f32
}
fn ceil(v: f32) -> f32 {
    f64::from(v).ceil() as f32
}

/// `0x001396c7`: the point of `anchor` in the HUD frame. 0 to 3 are the
/// frame's corners (top right, top left, bottom left, bottom right); 4 is
/// its centre, with `aim` in place of the centre's y when the view's aim
/// point projects (`scene_extract::hud::aim`), each rounded to a pixel.
pub fn anchor_point(anchor: u16, screen: &Screen, aim: Option<f32>) -> Result<[f32; 2], Refusal> {
    let [top, left, bottom, right] = screen.frame.map(f32::from);
    Ok(match anchor {
        0 => [right, top],
        1 => [left, top],
        2 => [left, bottom],
        3 => [right, bottom],
        4 => {
            // `cdq; sub eax, edx; sar eax, 1`: halved towards zero.
            let half = |a: i16, b: i16| ((i32::from(a) + i32::from(b)) / 2) as f32;
            let [top, left, bottom, right] = screen.frame;
            let x = half(left, right);
            let y = aim.unwrap_or(half(top, bottom));
            [floor(x + 0.5), floor(y + 0.5)]
        }
        _ => return Err(Refusal::Anchor),
    })
}

/// A sequence of a bitmap tag (`+0x40`, `0x3c` bytes each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sequence {
    /// `+0x20`.
    pub first_image: i16,
    /// `+0x22`.
    pub image_count: i16,
    /// `+0x34`.
    pub sprite_count: i32,
}

/// A sprite of a sequence (`+0x38`, `0x20` bytes each): its image and its
/// bounds in it (left, right, top, bottom, as fractions).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteRecord {
    pub image: i16,
    pub bounds: [f32; 4],
}

/// An image of a bitmap tag (`+0x48`, `0x74` bytes each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: i16,
    pub height: i16,
    /// Flags `+0x0e` bit `0x10`: the shader quad gives its texture
    /// coordinates in texels.
    pub texels: bool,
}

/// The bitmap tag as the sprite pick reads it. `None`: the read failed.
pub trait Bitmap {
    /// `+0x3c`.
    fn sequences(&self) -> Option<i32>;
    /// `+0x44`.
    fn images(&self) -> Option<i32>;
    fn sequence(&self, index: i32) -> Option<Sequence>;
    fn sprite(&self, sequence: i32, index: i32) -> Option<SpriteRecord>;
    fn image(&self, index: i32) -> Option<Image>;
}

/// What a widget draws of its bitmap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub image: i32,
    pub width: i32,
    pub height: i32,
    /// Left, right, top, bottom.
    pub bounds: [f32; 4],
}

const WHOLE: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// The image a widget's `sequence` names (`0x0022ae21`'s and
/// `0x0022b8e2`'s shared rule): the sequence's first sprite or image, or
/// its second when it has more than one and `second` is set; a bitmap
/// with no sequences gives image 0 for sequence 0. With the sprite's
/// bounds, or the whole image.
fn pick(bitmap: &dyn Bitmap, sequence: i32, second: bool) -> Result<Option<(i32, [f32; 4])>, Refusal> {
    if sequence < 0 {
        return Ok(None);
    }
    let count = bitmap.sequences().ok_or(Refusal::Read)?;
    if sequence >= count {
        let lone = sequence == 0 && count == 0 && bitmap.images().ok_or(Refusal::Read)? > 0;
        return Ok(lone.then_some((0, WHOLE)));
    }
    let s = bitmap.sequence(sequence).ok_or(Refusal::Read)?;
    if s.sprite_count > 0 {
        let which = i32::from(s.sprite_count > 1 && second);
        let sprite = bitmap.sprite(sequence, which).ok_or(Refusal::Read)?;
        return Ok(Some((i32::from(sprite.image), sprite.bounds)));
    }
    if s.image_count <= 0 {
        return Ok(None);
    }
    let which = i32::from(s.image_count > 1 && second);
    Ok(Some((i32::from(s.first_image) + which, WHOLE)))
}

/// `0x0022ae21`: the sprite of `sequence`, with its image's size.
pub fn sprite(bitmap: &dyn Bitmap, sequence: i32, second: bool) -> Result<Option<Sprite>, Refusal> {
    let Some((image, bounds)) = pick(bitmap, sequence, second)? else { return Ok(None) };
    let size = bitmap.image(image).ok_or(Refusal::Read)?;
    Ok(Some(Sprite { image, width: i32::from(size.width), height: i32::from(size.height), bounds }))
}

/// The basis a widget is laid out on: its point and the two axes its art
/// spans. The widget's effects (`0x0022bcaf`) change it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basis {
    pub origin: [f32; 2],
    pub x: [f32; 2],
    pub y: [f32; 2],
}

/// A widget's effects: the block `(count, address)`, the widget's inputs
/// (already in the shader inputs, which the effects' functions read) and
/// the basis to change.
pub type Effects<'a> = &'a dyn Fn((u32, u32), &[f32; 4], &mut Basis) -> Result<(), Refusal>;

/// What the special types read outside the record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Art {
    /// [`Session::user_state`].
    pub user_state: bool,
    /// The player's UI record (`[0x00510c4c] + player * 0x6c`): the
    /// meter's level `+0` and its flash `+4`.
    pub meter: [f32; 2],
    /// The shield meter's colour (`[0x00468710]` points at it), as bits.
    pub shield_colour: [u32; 3],
    /// The list type's second colour (`[0x00468718]` points at it).
    pub list_colour: [u32; 3],
    /// A sprite that replaces the bitmap's own pick (`0x0022ae21`): what a
    /// feature that changes a widget's art gives the draw.
    pub sprite: Option<Sprite>,
}

/// One call of the shader quad `0x0022a664`.
#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    /// The globals written since the last quad, as (address, word).
    pub writes: Vec<(u32, u32)>,
    /// Top left, top right, bottom right, bottom left.
    pub corners: [[f32; 2]; 4],
    /// Left, right, top, bottom of the image.
    pub bounds: [f32; 4],
}

/// A bitmap widget's draw.
#[derive(Clone, Debug, PartialEq)]
pub struct BitmapDraw {
    pub anchor: u16,
    /// The anchor's point, before the widget's offset: what the group's
    /// fit is taken at (`canvas_fit::anchor_fit`).
    pub point: [f32; 2],
    pub bitmap: u32,
    pub shader: u32,
    pub sprite: Sprite,
    pub quads: Vec<Quad>,
    /// Globals written after the last quad's.
    pub writes: Vec<(u32, u32)>,
}

/// `0x00131bb0`: a packed colour's red, green and blue as floats.
fn rgb(colour: u32) -> [u32; 3] {
    const SCALE: f32 = f32::from_bits(0x3b80_8081);
    [16, 8, 0].map(|shift| (((colour >> shift) & 0xff) as f32 * SCALE).to_bits())
}

fn write3(writes: &mut Vec<(u32, u32)>, at: u32, words: [u32; 3]) {
    writes.extend(words.into_iter().enumerate().map(|(i, w)| (at + 4 * i as u32, w)));
}

/// `v` held to 0..1 as the meter's compares do (a NaN passes).
#[allow(clippy::manual_clamp)]
fn unit(v: f32) -> f32 {
    if 0.0 > v {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// `0x0022aff1`: the bitmap widget `w` drawn with `inputs`. `Ok(None)`:
/// the widget draws nothing (no bitmap, shader or image).
#[allow(clippy::too_many_arguments)]
pub fn bitmap_widget(
    w: &[u8],
    inputs: &[f32; 4],
    record: &[u8],
    screen: &Screen,
    aim: Option<f32>,
    bitmap: &dyn Bitmap,
    art: &Art,
    effects: Effects<'_>,
) -> Result<Option<BitmapDraw>, Refusal> {
    if w.len() < BITMAP_SIZE || record.len() < RECORD_SIZE {
        return Err(Refusal::Read);
    }
    let anchor = u16_at(w, 0x1c);
    let mode = mode(anchor, screen).index();
    let (tag, shader) = (u32_at(w, 0x24), u32_at(w, 0x2c));
    if tag == u32::MAX || shader == u32::MAX {
        return Ok(None);
    }
    let sequence = i32::from(w[0x30 + mode] as i8);
    let sprite = match art.sprite {
        Some(given) => given,
        None => match sprite(bitmap, sequence, art.user_state)? {
            Some(picked) => picked,
            None => return Ok(None),
        },
    };
    let point = anchor_point(anchor, screen, aim)?;
    let [mut left, mut right, mut top, mut bottom] = sprite.bounds;
    let offset = 0x34 + 4 * mode;
    let origin = [f32::from(i16_at(w, offset)) + point[0], f32::from(i16_at(w, offset + 2)) + point[1]];
    let registration = 0x40 + 8 * mode;
    let reg_x = (right - left) * f32_at(w, registration) + left;
    let reg_y = (bottom - top) * f32_at(w, registration + 4) + top;
    let mut pending: Vec<(u32, u32)> = inputs.iter().enumerate().map(|(i, v)| (global::INPUTS + 4 * i as u32, v.to_bits())).collect();
    let mut basis = Basis { origin, x: [1.0, 0.0], y: [0.0, 1.0] };
    effects((u32_at(w, 0x58), u32_at(w, 0x5c)), inputs, &mut basis)?;
    let (height, width) = (sprite.height as f32, sprite.width as f32);
    let mut corners = [[0.0f32; 2]; 4];
    for (i, corner) in corners.iter_mut().enumerate() {
        let dy = if i > 1 { ceil((bottom - reg_y) * height) } else { -floor((reg_y - top) * height) };
        let dx = if i == 1 || i == 2 { ceil((right - reg_x) * width) } else { -floor((reg_x - left) * width) };
        *corner = [
            basis.x[0] * dx + basis.y[0] * dy + basis.origin[0],
            basis.x[1] * dx + basis.y[1] * dy + basis.origin[1],
        ];
    }
    let flags = u16_at(w, 0x1e);
    if flags & 1 != 0 {
        std::mem::swap(&mut left, &mut right);
    }
    if flags & 2 != 0 {
        std::mem::swap(&mut top, &mut bottom);
    }
    let mut quads = Vec::new();
    let mut special = false;
    match u16_at(w, 0x60) {
        1 | 2 => return Err(Refusal::Emblem),
        kind @ (3 | 4) => {
            let colour = if kind == 3 { 0x18c } else { 0x1d8 };
            let words = [0, 4, 8].map(|o| u32_at(record, colour + o));
            write3(&mut pending, global::COLOUR_A, words);
            write3(&mut pending, global::COLOUR_B, words);
        }
        5 => {
            special = true;
            const LAYERS: [u32; 5] = [0, 0x00ff_0000, 0x0000_ff00, 0x00ff_ff00, 0x007f_00ff];
            for (k, layer) in LAYERS.into_iter().enumerate() {
                let at = k as f32;
                let level = unit(f32_at(record, 0x2c) - at);
                let mut shown = unit(art.meter[0] - at);
                let falling = shown > level;
                let flash = if falling { art.meter[1] } else { -1.0 };
                let fade = if 0.0 > flash { 0.0 } else { unit(1.0 - flash) };
                let colour = art.shield_colour.map(|c| (f32::from_bits(c) * fade).to_bits());
                if !falling {
                    shown = level;
                }
                // `comiss 0, level; jb` draws; else `comiss 0, shown; jae` ends
                // the layers: an unordered compare draws.
                if 0.0 >= level && 0.0 >= shown {
                    break;
                }
                pending.push((global::INPUTS, shown.to_bits()));
                pending.push((global::INPUTS + 4, level.to_bits()));
                write3(&mut pending, global::COLOUR_A, colour);
                let (second, third) = match (k, art.user_state) {
                    (0, true) => (0x0045_059a, 0x009c_46c1),
                    (0, false) => (0x0000_55aa, 0x0000_78f0),
                    _ => (layer, layer),
                };
                write3(&mut pending, global::COLOUR_B, rgb(second));
                write3(&mut pending, global::COLOUR_C, rgb(third));
                quads.push(Quad { writes: std::mem::take(&mut pending), corners, bounds: [left, right, top, bottom] });
            }
        }
        7 => {
            special = true;
            let count = u32_at(record, 0x22c) as i32;
            if count > 0 {
                if count as usize > (RECORD_SIZE - 0x254) / 4 {
                    return Err(Refusal::Read);
                }
                let step = (sprite.width + 1) as f32;
                let shift = (count - 1) as f32 * (step * 0.5);
                for corner in &mut corners {
                    corner[0] -= shift;
                }
                write3(&mut pending, global::COLOUR_B, art.list_colour);
                for i in 0..count as usize {
                    write3(&mut pending, global::COLOUR_A, rgb(u32_at(record, 0x230 + 4 * i)));
                    pending.push((global::INPUTS, u32_at(record, 0x250 + 4 * i)));
                    quads.push(Quad { writes: std::mem::take(&mut pending), corners, bounds: [left, right, top, bottom] });
                    for corner in &mut corners {
                        corner[0] += step;
                    }
                }
            }
        }
        _ => {}
    }
    if flags & 0x10 != 0 {
        let (across, down) = (corners[1][0] - corners[0][0], corners[2][1] - corners[1][1]);
        for (i, corner) in corners.iter_mut().enumerate() {
            if i != 1 && i != 2 {
                corner[0] -= across * 10.0;
            }
            if i <= 1 {
                corner[1] -= down * 10.0;
            }
        }
        left -= (right - left) * 10.0;
        top -= (bottom - top) * 10.0;
    }
    if !special {
        let bounds = [left, right, top, bottom];
        quads.push(Quad { writes: std::mem::take(&mut pending), corners, bounds });
        let (across, down) = (corners[1][0] - corners[0][0], corners[2][1] - corners[1][1]);
        let mirrored = |x: bool, y: bool| {
            let mut c = corners;
            for (i, corner) in c.iter_mut().enumerate() {
                if x && i != 1 && i != 2 {
                    corner[0] += across * 2.0;
                }
                if y && i <= 1 {
                    corner[1] += down * 2.0;
                }
            }
            Quad { writes: Vec::new(), corners: c, bounds }
        };
        if flags & 4 != 0 {
            quads.push(mirrored(true, false));
        }
        if flags & 8 != 0 {
            quads.push(mirrored(false, true));
        }
        if flags & 4 != 0 && flags & 8 != 0 {
            quads.push(mirrored(true, true));
        }
    }
    Ok(Some(BitmapDraw { anchor, point, bitmap: tag, shader, sprite, quads, writes: pending }))
}

/// The depth and w the shader quad gives every vertex (`[0x0045dd40]`).
pub const QUAD_DEPTH: f32 = 16_777_215.0;

/// `0x0022a664` up to its state setup: the quad's four vertices as x, y,
/// z, w, u, v (the diffuse colour is white), in the corners' order.
pub fn vertices(corners: &[[f32; 2]; 4], bounds: &[f32; 4], image: &Image) -> [[f32; 6]; 4] {
    let (sx, sy) = if image.texels { (f32::from(image.width), f32::from(image.height)) } else { (1.0, 1.0) };
    let [left, right, top, bottom] = *bounds;
    let (u0, u1, v0, v1) = (left * sx, right * sx, top * sy, bottom * sy);
    let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
    std::array::from_fn(|i| [corners[i][0], corners[i][1], QUAD_DEPTH, QUAD_DEPTH, uv[i][0], uv[i][1]])
}

/// A screen-effect widget's call of `0x0002d140`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenDraw {
    /// The mode's screen effect (`egor`) datum.
    pub effect: u32,
    /// The bitmap and image the widget sets (`0x004e6978`, `0x004e697c`).
    pub bitmap: u32,
    pub image: i32,
    /// The four values the call takes: the widget's inputs.
    pub values: [f32; 4],
    /// Three rows of three corners, each x, y, u, v: the window's edges
    /// and the widget's point.
    pub grid: [[f32; 4]; 9],
}

/// `0x0022b8e2`: the screen-effect widget `w` drawn with `inputs`.
/// `Ok(None)`: the mode has no effect.
pub fn screen_widget(
    w: &[u8],
    inputs: &[f32; 4],
    screen: &Screen,
    aim: Option<f32>,
    bitmap: Option<&dyn Bitmap>,
    user_state: bool,
) -> Result<Option<ScreenDraw>, Refusal> {
    if w.len() < SCREEN_SIZE {
        return Err(Refusal::Read);
    }
    let anchor = u16_at(w, 0x1c);
    let mode = mode(anchor, screen).index();
    let effect = u32_at(w, 0x2c + 8 * mode);
    if effect == u32::MAX {
        return Ok(None);
    }
    let tag = u32_at(w, 0x24);
    let image = match bitmap {
        Some(bitmap) if tag != u32::MAX => pick(bitmap, i32::from(w[0x40 + mode] as i8), user_state)?.map_or(-1, |(image, _)| image),
        _ => -1,
    };
    let point = anchor_point(anchor, screen, aim)?;
    let offset = 0x44 + 4 * mode;
    let (px, py) = (f32::from(i16_at(w, offset)) + point[0], f32::from(i16_at(w, offset + 2)) + point[1]);
    let [top, left, bottom, right] = screen.window;
    let sx = 0.5 / (i32::from(right) - i32::from(left)) as f32;
    let sy = 0.5 / (i32::from(bottom) - i32::from(top)) as f32;
    let [top, left, bottom, right] = screen.window.map(f32::from);
    let u_left = 1.0 - (px - left) * sx;
    let u_right = 1.0 - (right - px) * sx;
    let v_top = 1.0 - (py - top) * sy;
    let v_bottom = 1.0 - (bottom - py) * sy;
    let (xs, us) = ([left, px, right], [u_left, 1.0, u_right]);
    let (ys, vs) = ([top, py, bottom], [v_top, 1.0, v_bottom]);
    let grid = std::array::from_fn(|i| [xs[i % 3], ys[i / 3], us[i % 3], vs[i / 3]]);
    Ok(Some(ScreenDraw { effect, bitmap: tag, image, values: *inputs, grid }))
}

/// Where a text widget's string goes (`0x0022aa16`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextPlace {
    /// The anchor and its point, before the widget's offset: what the
    /// group's fit is taken at.
    pub anchor: u16,
    pub point: [f32; 2],
    pub shader: u32,
    /// The string ID, or the input's name for a number (`flags` bit 1).
    pub string: u32,
    pub flags: u16,
    /// 0 starts at the point, 1 centres on it, 2 ends at it.
    pub justification: u16,
    /// The engine font (`0x0022a85b`: byte 0 is font 6, 1 font 8).
    pub font: Option<i16>,
    /// The basis after the widget's effects, in the frame's pixels.
    pub basis: Basis,
}

/// `0x0022aa16` up to its string: the text widget `w`'s place. `Ok(None)`:
/// no string or no shader.
pub fn text_widget(
    w: &[u8],
    inputs: &[f32; 4],
    screen: &Screen,
    aim: Option<f32>,
    effects: Effects<'_>,
) -> Result<Option<TextPlace>, Refusal> {
    if w.len() < TEXT_SIZE {
        return Err(Refusal::Read);
    }
    let (shader, string) = (u32_at(w, 0x24), u32_at(w, 0x28));
    if string == 0 || shader == u32::MAX {
        return Ok(None);
    }
    let anchor = u16_at(w, 0x1c);
    let mode = mode(anchor, screen).index();
    let font = match w[0x30 + mode] as i8 {
        0 => Some(6),
        1 => Some(8),
        _ => None,
    };
    let point = anchor_point(anchor, screen, aim)?;
    let offset = 0x40 + 4 * mode;
    let origin = [f32::from(i16_at(w, offset)) + point[0], f32::from(i16_at(w, offset + 2)) + point[1]];
    let mut basis = Basis { origin, x: [1.0, 0.0], y: [0.0, 1.0] };
    effects((u32_at(w, 0x4c), u32_at(w, 0x50)), inputs, &mut basis)?;
    Ok(Some(TextPlace { anchor, point, shader, string, flags: u16_at(w, 0x1e), justification: u16_at(w, 0x2c), font, basis }))
}

/// Text flags (`+0x1e`): the string is a number, the input named at
/// `+0x28` truncated; two or three digits wide; a player's name.
pub const TEXT_NUMBER: u16 = 1;
pub const TEXT_TWO_DIGITS: u16 = 2;
pub const TEXT_THREE_DIGITS: u16 = 4;
pub const TEXT_NAME: u16 = 8;

/// `0x0022a8b0`..`0x0022a929`: a number widget's string. Two digits hold
/// the value to 0..99 and pad it with a zero (`0%d`); three hold it to
/// 0..999 and pad it with zeros (`00%d`, `0%d`); neither prints it as it
/// is (`%d`).
pub fn number_text(flags: u16, value: i32) -> String {
    if flags & TEXT_TWO_DIGITS != 0 {
        format!("{:02}", value.clamp(0, 99))
    } else if flags & TEXT_THREE_DIGITS != 0 {
        format!("{:03}", value.clamp(0, 999))
    } else {
        value.to_string()
    }
}

/// `cvttss2si` to a short: the low word of the truncated value, or of
/// the integer indefinite when it is outside an `i32`.
fn short(v: f32) -> i16 {
    let wide = if v.is_nan() || v >= 2_147_483_648.0 || v < -2_147_483_648.0 { i32::MIN } else { v as i32 };
    wide as i16
}

/// `0x0013e8e0`, the measure's callback over each glyph's rectangle
/// (`[x0, y0, x1, y1]`, laid out in the rectangle 0, 0, 1000, 1000): the
/// bounds top, left, bottom, right, the far edges one past the glyph.
/// `None` with no glyph.
pub fn text_bounds(glyphs: &[[f32; 4]]) -> Option<[i16; 4]> {
    if glyphs.is_empty() {
        return None;
    }
    let (mut top, mut left, mut bottom, mut right) = (i16::MAX, i16::MAX, i16::MIN, i16::MIN);
    for &[x0, y0, x1, y1] in glyphs {
        let (far_x, far_y) = (x1 + 1.0, y1 + 1.0);
        if f32::from(left) > x0 {
            left = short(x0);
        }
        if f32::from(top) > y0 {
            top = short(y0);
        }
        if far_x > f32::from(right) {
            right = short(far_x);
        }
        if far_y > f32::from(bottom) {
            bottom = short(far_y);
        }
    }
    Some([top, left, bottom, right])
}

/// `0x0022ab7d`..`0x0022ac94`: the rectangle the string is drawn in,
/// top, left, bottom, right in the window's pixels, from the basis's
/// `origin` (the frame's pixels), the window and the measured `bounds`.
/// Justification 0 starts at the origin, 1 centres on it, 2 ends at it;
/// any other keeps the measure's rectangle.
pub fn text_rect(justification: u16, origin: [f32; 2], window: &[i16; 4], bounds: &[i16; 4]) -> [i16; 4] {
    let (px, py) = (origin[0] - f32::from(window[1]), origin[1] - f32::from(window[0]));
    let width = i32::from(bounds[3].wrapping_sub(bounds[1]));
    let height = i32::from(bounds[2].wrapping_sub(bounds[0]));
    let bottom = || short(height as f32 + py);
    match justification {
        0 => [short(py), short(px), bottom(), short(width as f32 + px)],
        1 => {
            let half = width >> 1;
            [short(py), short(px - half as f32), bottom(), short((width - half) as f32 + px)]
        }
        2 => [short(py), short(px - width as f32), bottom(), short(px)],
        _ => [0, 0, 1000, 1000],
    }
}

/// `0x0013f4ab`..`0x0013f567`: the string's colour and its shadow's, as
/// the glyph quads carry them (alpha, red, green, blue from the top
/// byte), from the two colour blocks the text widget sets (`0x0013eb60`
/// with `[0x004686cc]`, `0x0013ed50` with `[0x004686d4]`). The shadow's
/// alpha is the lesser of its own and the string's, and it is drawn
/// while that is not 0. `None`: a component outside 0..1, which
/// `0x0013eb60` does not take.
pub fn text_colours(main: &[f32; 4], shadow: Option<&[f32; 4]>) -> Option<(u32, Option<u32>)> {
    let unit = |c: &[f32; 4]| c.iter().all(|&v| (0.0..=1.0).contains(&v));
    // `fmul` by 255.0 at double precision, then `_ftol`.
    let pack = |c: [f32; 4]| c.iter().fold(0u32, |word, &v| word << 8 | (f64::from(v) * 255.0) as u32);
    if !unit(main) {
        return None;
    }
    let shadow = match shadow {
        None => None,
        Some(s) if !unit(s) => return None,
        Some(s) => {
            let alpha = if s[0] > main[0] { main[0] } else { s[0] };
            Some(pack([alpha, s[1], s[2], s[3]])).filter(|word| word & 0xff00_0000 != 0)
        }
    };
    Some((pack(*main), shadow))
}

/// One glyph quad of a string, as `0x0001f490` draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    /// The glyph's index in its font.
    pub index: u32,
    /// `[x0, y0, x1, y1]` in the window's pixels.
    pub rect: [f32; 4],
    /// Alpha, red, green, blue from the top byte.
    pub colour: u32,
}

/// `0x0013f660`: whether the string draw gives the glyph of `unit` white
/// in place of the string's colour (`0x0013f9c9`: `or edx, 0x00ffffff`
/// before the glyph's call). The private-use characters `0xe000` to
/// `0xe3ff` are, but for `0xe000` to `0xe002`, `0xe004`, `0xe008` and
/// `0xe106` to `0xe131`: the coloured buttons `0xe100` to `0xe105` keep
/// their own colours, the sticks and triggers after them take the string's.
pub fn untinted(unit: u16) -> bool {
    matches!(unit, 0xe003 | 0xe005..=0xe007 | 0xe009..=0xe105 | 0xe132..=0xe3ff)
}

/// `0x0001f490` over the laid-out glyphs: each glyph's shadow, one pixel
/// right and down, then the glyph. A glyph whose index is in `white`
/// (an [`untinted`] character's) is white at the string's alpha.
pub fn text_quads(glyphs: &[(u32, [f32; 4])], white: &[u32], main: u32, shadow: Option<u32>) -> Vec<GlyphQuad> {
    let mut out = Vec::with_capacity(glyphs.len() * 2);
    for &(index, rect) in glyphs {
        if let Some(colour) = shadow {
            out.push(GlyphQuad { index, rect: rect.map(|v| v + 1.0), colour });
        }
        let colour = if white.contains(&index) { main | 0x00ff_ffff } else { main };
        out.push(GlyphQuad { index, rect, colour });
    }
    out
}

/// The shader colour the text widget sets before its shader
/// (`0x0022aabb`: `0x00131bb0` to `0x004e6950`).
pub const TEXT_COLOUR: u32 = 0x004e_6950;

/// The globals a text widget's draw leaves that a HUD shader reads: its
/// inputs (`0x0022a898`) and the HUD's colour `tint` (`[0x00510c94] +
/// 0xa4`, packed) as floats.
pub fn text_globals(inputs: &[f32; 4], tint: u32) -> Vec<(u32, u32)> {
    let mut writes: Vec<(u32, u32)> = inputs.iter().enumerate().map(|(i, v)| (global::INPUTS + 4 * i as u32, v.to_bits())).collect();
    write3(&mut writes, TEXT_COLOUR, rgb(tint));
    writes
}

/// A definition's three widget blocks, each a whole number of widgets.
#[derive(Clone, Copy, Debug, Default)]
pub struct Definition<'a> {
    pub screens: &'a [u8],
    pub bitmaps: &'a [u8],
    pub texts: &'a [u8],
}

/// What a walk item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Screen,
    Bitmap,
    Text,
}

impl Kind {
    /// The kind's widget size.
    pub fn size(self) -> usize {
        match self {
            Kind::Screen => SCREEN_SIZE,
            Kind::Bitmap => BITMAP_SIZE,
            Kind::Text => TEXT_SIZE,
        }
    }
}

/// A widget the walk draws: its place in its block and its inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub kind: Kind,
    pub index: usize,
    pub inputs: [f32; 4],
}

/// `0x0013a532`: the widgets the walk draws, in its order (screen
/// effects, then bitmaps, then texts, each in the definition's), with
/// the inputs each is drawn with.
pub fn shown(definition: &Definition<'_>, words: &Words, record: &[u8], session: &Session) -> Vec<Shown> {
    let mut out = Vec::new();
    for (kind, block) in [(Kind::Screen, definition.screens), (Kind::Bitmap, definition.bitmaps), (Kind::Text, definition.texts)] {
        for (index, w) in block.chunks_exact(kind.size()).enumerate() {
            if gate(&w[8..0x1c], words, record) {
                out.push(Shown { kind, index, inputs: inputs(&w[4..8], record, words.weapon, session) });
            }
        }
    }
    out
}

/// One walk `0x001394b9` makes: the record word that names the
/// definition, the definition's datum and the walk's slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefinitionWalk {
    /// Its place in [`DEFINITION_ORDER`].
    pub step: usize,
    /// The record's word: the datum is at `4 * word`.
    pub word: usize,
    pub datum: u32,
    pub slot: u32,
}

/// `0x001394b9`'s walks in its order, as (record word, slot): word 5 with
/// slot 3, word 2 with 4, 3 with 1, 4 with 2, word 5 with slot 3 again,
/// then words 6, 0 and 1 with slot 0 (Code, `h2 disasm 0x001394b9`).
pub const DEFINITION_ORDER: [(usize, u32); 8] = [(5, 3), (2, 4), (3, 1), (4, 2), (5, 3), (6, 0), (0, 0), (1, 0)];

/// `0x001394b9`: the definitions the HUD `record` names, in the order the
/// guest walks them; a word of `-1` names none and is skipped.
pub fn definitions(record: &[u8]) -> Option<Vec<DefinitionWalk>> {
    if record.len() < 7 * 4 {
        return None;
    }
    Some(
        DEFINITION_ORDER
            .iter()
            .enumerate()
            .map(|(step, &(word, slot))| DefinitionWalk { step, word, datum: u32_at(record, 4 * word), slot })
            .filter(|w| w.datum != u32::MAX)
            .collect(),
    )
}
