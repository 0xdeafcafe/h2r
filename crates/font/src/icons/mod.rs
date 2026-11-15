//! Button-icon glyphs for other controllers, drawn in code.
//!
//! Halo 2's fonts carry the Xbox pad's buttons as glyphs at private-use code
//! points ([`BUTTON_UNITS`]). This module draws icons for other devices: Xbox
//! 360 bumpers, PlayStation face and shoulder buttons, and keyboard keys and
//! mouse buttons. [`replacements`] turns the sets into [`Replacement`]s for
//! one font, sized from that font's own icons, ready for
//! [`crate::patch::apply`]. Each set goes at code units of its own
//! ([`alias_unit`]), next to the retail glyphs, so one font holds every set
//! and the runner picks one per character drawn ([`shown_unit`]).
//!
//! Every pixel here is computed from simple shapes and a 5x7 letter set made
//! for this project. No retail glyph and no vendor artwork is stored or
//! copied. `docs/button-icons.md` has the design and the style rules.
use std::collections::BTreeMap;
use std::str::FromStr;

use crate::Font;
use crate::patch::Replacement;

pub mod canvas;
pub mod label;

use canvas::{
    Canvas, Rgba, both, circle, inner_outline, outline, polygon, round_rect, segment, white,
};

/// A control of the Xbox pad that has an icon glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    A,
    B,
    X,
    Y,
    Black,
    White,
    LeftTrigger,
    RightTrigger,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Start,
    Back,
    LeftStick,
    RightStick,
    LeftStickClick,
    RightStickClick,
}

/// The code unit of each button's glyph, the same in every font on the disc.
pub const BUTTON_UNITS: [(u16, Button); 18] = [
    (0xe100, Button::A),
    (0xe101, Button::B),
    (0xe102, Button::X),
    (0xe103, Button::Y),
    (0xe104, Button::Black),
    (0xe105, Button::White),
    (0xe106, Button::LeftTrigger),
    (0xe107, Button::RightTrigger),
    (0xe108, Button::DpadUp),
    (0xe109, Button::DpadDown),
    (0xe10a, Button::DpadLeft),
    (0xe10b, Button::DpadRight),
    (0xe10c, Button::Start),
    (0xe10d, Button::Back),
    (0xe10e, Button::LeftStick),
    (0xe10f, Button::RightStick),
    (0xe110, Button::LeftStickClick),
    (0xe111, Button::RightStickClick),
];

/// Which controller the button glyphs depict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSet {
    /// The retail glyphs, untouched.
    Xbox,
    Xbox360,
    PlayStation,
    Keyboard,
}

/// The `icons` setting: a fixed set, or one picked from the input devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconChoice {
    #[default]
    Auto,
    Set(IconSet),
}

impl FromStr for IconChoice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "auto" => IconChoice::Auto,
            "xbox" => IconChoice::Set(IconSet::Xbox),
            "xbox360" => IconChoice::Set(IconSet::Xbox360),
            "playstation" => IconChoice::Set(IconSet::PlayStation),
            "keyboard" => IconChoice::Set(IconSet::Keyboard),
            _ => {
                return Err(format!(
                    "unknown icon set {s:?} (want auto, xbox, xbox360, playstation or keyboard)"
                ));
            }
        })
    }
}

impl IconChoice {
    /// The set to use, decided once. `Auto` goes by the first physical pad's
    /// name when the host knows it (a Sony pad gets PlayStation icons, any
    /// other pad Xbox 360). Without a name it starts on the keyboard when
    /// present, then a modern pad if enabled, otherwise the retail glyphs.
    pub fn resolve(self, pad_name: Option<&str>, gamepads: bool, keyboard: bool) -> IconSet {
        match (self, pad_name) {
            (IconChoice::Set(set), _) => set,
            (IconChoice::Auto, Some(name)) => {
                let n = name.to_ascii_lowercase();
                let sony = [
                    "dualshock",
                    "dualsense",
                    "playstation",
                    "sony",
                    "ps4",
                    "ps5",
                ];
                if sony.iter().any(|s| n.contains(s)) {
                    IconSet::PlayStation
                } else {
                    IconSet::Xbox360
                }
            }
            (IconChoice::Auto, None) if keyboard => IconSet::Keyboard,
            (IconChoice::Auto, None) if gamepads => IconSet::Xbox360,
            (IconChoice::Auto, None) => IconSet::Xbox,
        }
    }
}

/// What the keyboard set draws for a button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyIcon {
    /// A keycap with this label (see [`key_label`]).
    Key(String),
    Mouse(MouseIcon),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseIcon {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// Moving the mouse (look).
    Move,
}

/// The keyboard set's icon per button. A button with no entry keeps its
/// retail glyph.
pub type KeyLabels = BTreeMap<Button, KeyIcon>;

/// Longest keycap label drawn; longer ones are cut.
pub const MAX_LABEL_CHARS: usize = 12;

/// The keycap label for a canonical key name of `crates/input`
/// (`"w"`, `"space"`, `"arrowup"`, `"controlleft"`, `"f5"`).
pub fn key_label(key: &str) -> String {
    let k = key.to_ascii_lowercase();
    match k.as_str() {
        "space" => "SPACE",
        "enter" => "ENTER",
        "escape" => "ESC",
        "tab" => "TAB",
        "backspace" => "BKSP",
        "capslock" => "CAPS",
        "delete" => "DEL",
        "insert" => "INS",
        "home" => "HOME",
        "end" => "END",
        "pageup" => "PGUP",
        "pagedown" => "PGDN",
        "arrowup" => "\u{2191}",
        "arrowdown" => "\u{2193}",
        "arrowleft" => "\u{2190}",
        "arrowright" => "\u{2192}",
        "shiftleft" => "SHIFT",
        "shiftright" => "RSHIFT",
        "controlleft" => "CTRL",
        "controlright" => "RCTRL",
        "altleft" => "ALT",
        "altright" => "RALT",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "semicolon" => ";",
        "quote" => "'",
        "bracketleft" => "[",
        "bracketright" => "]",
        "backslash" => "\\",
        "minus" => "-",
        "equal" => "=",
        "backquote" => "`",
        _ => return k.to_ascii_uppercase(),
    }
    .to_owned()
}

/// A drawn icon: ARGB4444, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u16>,
}

/// Smallest and largest cell height drawn; other heights are clamped.
pub const MIN_HEIGHT: u16 = 9;
pub const MAX_HEIGHT: u16 = 64;

/// First code unit of each set's glyphs. A button's glyph in a set is at the
/// set's base plus the button's offset from U+E100. These ranges are inside
/// the engine's icon range (U+E000 to U+E3FF) and hold no glyph in any font
/// on the retail disc.
pub const ALIAS_BASES: [(IconSet, u16); 3] = [
    (IconSet::Xbox360, 0xe200),
    (IconSet::PlayStation, 0xe220),
    (IconSet::Keyboard, 0xe240),
];

/// The retail code unit of the first button glyph.
pub const FIRST_BUTTON_UNIT: u16 = 0xe100;

/// Where `set` keeps its glyph for the button at retail `unit`.
pub fn alias_unit(set: IconSet, unit: u16) -> u16 {
    match ALIAS_BASES.iter().find(|(s, _)| *s == set) {
        Some((_, base)) => base + (unit - FIRST_BUTTON_UNIT),
        None => unit,
    }
}

/// Whether `set` has its own picture for `button`. Where it does not, the
/// retail glyph is right for that device and stays.
pub fn replaces(set: IconSet, button: Button, keys: &KeyLabels) -> bool {
    use Button::*;
    match set {
        IconSet::Xbox => false,
        // The rest of a 360 pad matches the retail pictures: coloured A, B,
        // X and Y, a trigger pair, sticks, and Start and Back as triangles.
        IconSet::Xbox360 => matches!(button, Black | White),
        // Sticks and the d-pad retain their device-independent pictures.
        IconSet::PlayStation => matches!(
            button,
            A | B | X | Y | Black | White | LeftTrigger | RightTrigger | Start | Back
        ),
        IconSet::Keyboard => keys.contains_key(&button),
    }
}

/// The code unit to draw when the text has `unit` and `set` is showing:
/// the set's own glyph for a button it replaces, otherwise `unit` itself.
pub fn shown_unit(set: IconSet, unit: u16, keys: &KeyLabels) -> u16 {
    match BUTTON_UNITS.iter().find(|(u, _)| *u == unit) {
        Some((_, button)) if replaces(set, *button, keys) => alias_unit(set, unit),
        _ => unit,
    }
}

/// The icon `set` shows for `button` in a cell `height` pixels tall, or
/// `None` where the set keeps the retail glyph ([`replaces`]).
pub fn icon(set: IconSet, button: Button, height: u16, keys: &KeyLabels) -> Option<Icon> {
    use Button::*;
    if !replaces(set, button, keys) {
        return None;
    }
    let h = usize::from(height.clamp(MIN_HEIGHT, MAX_HEIGHT));
    let (canvas, mono) = match (set, button) {
        // Right bumper is Black and left is White, as crates/input maps them.
        (IconSet::Xbox360, Black) => (shoulder_pair(h, Side::Right, ["LB", "RB"], false), true),
        (IconSet::Xbox360, White) => (shoulder_pair(h, Side::Left, ["LB", "RB"], false), true),
        (IconSet::PlayStation, Black) => (shoulder_pair(h, Side::Right, ["L1", "R1"], false), true),
        (IconSet::PlayStation, White) => (shoulder_pair(h, Side::Left, ["L1", "R1"], false), true),
        (IconSet::PlayStation, LeftTrigger) => {
            (shoulder_pair(h, Side::Left, ["L2", "R2"], true), true)
        }
        (IconSet::PlayStation, RightTrigger) => {
            (shoulder_pair(h, Side::Right, ["L2", "R2"], true), true)
        }
        (IconSet::PlayStation, A) => (face(h, Symbol::Cross), false),
        (IconSet::PlayStation, B) => (face(h, Symbol::Circle), false),
        (IconSet::PlayStation, X) => (face(h, Symbol::Square), false),
        (IconSet::PlayStation, Y) => (face(h, Symbol::Triangle), false),
        (IconSet::PlayStation, Start) => (face(h, Symbol::Options), false),
        (IconSet::PlayStation, Back) => (face(h, Symbol::Share), false),
        (IconSet::Keyboard, b) => match keys.get(&b)? {
            KeyIcon::Key(label) => {
                let label: String = label.chars().take(MAX_LABEL_CHARS).collect();
                (keycap(h, &label), false)
            }
            KeyIcon::Mouse(m) => (mouse(h, *m), false),
        },
        _ => return None,
    };
    Some(Icon {
        width: canvas.width as u16,
        height: canvas.height as u16,
        pixels: if mono {
            canvas.to_white_argb4444()
        } else {
            canvas.to_argb4444()
        },
    })
}

/// Whether the code units the sets use are unused in `font`: unmapped, a
/// record with no bitmap, or the filler record many private-use units share.
pub fn alias_range_free(font: &Font<'_>) -> bool {
    // The retail icon range ends in a uniform filler tail. Merely sharing a
    // record is not enough: a real symbol can have several Unicode aliases.
    let filler = font.glyph_index(0xe3ff);
    let uniform_tail = filler == font.glyph_index(0xffff)
        && (0xe300..=0xe3ff).all(|u| font.glyph_index(u) == filler);
    let button_filler = BUTTON_UNITS
        .iter()
        .any(|(u, _)| font.glyph_index(*u) == filler);
    ALIAS_BASES.iter().all(|(_, base)| {
        (*base..*base + BUTTON_UNITS.len() as u16).all(|unit| {
            let index = font.glyph_index(unit);
            index == 0
                || (uniform_tail && !button_filler && index == filler)
                || font
                    .record(index)
                    .is_ok_and(|r| r.width == 0 || r.height == 0)
        })
    })
}

/// Replacements that add every set to `font`, each at its own code units.
/// An icon takes the height and vertical offset of the retail glyph it
/// stands for in this font, so it sits where that one sits; the advance is
/// the new width, as on the retail icons. Buttons the font has no glyph for
/// are skipped, and a font that already uses the sets' code units is left
/// alone (the result is empty).
pub fn replacements(font: &Font<'_>, keys: &KeyLabels) -> Vec<Replacement> {
    let mut out = Vec::new();
    if !alias_range_free(font) {
        return out;
    }
    for (set, _) in ALIAS_BASES {
        for (unit, button) in BUTTON_UNITS {
            let index = font.glyph_index(unit);
            let Ok(record) = font.record(index) else {
                continue;
            };
            if index == 0 || record.width == 0 || record.height == 0 {
                continue;
            }
            if let Some(icon) = icon(set, button, record.height, keys) {
                out.push(Replacement {
                    unit: alias_unit(set, unit),
                    width: icon.width,
                    height: icon.height,
                    advance: icon.width as i16,
                    x_offset: record.x_offset,
                    y_offset: Some(record.y_offset),
                    pixels: icon.pixels,
                });
            }
        }
    }
    out
}

// Opacities of the monochrome pictures, taken from the levels the retail
// trigger and stick pairs use: the idle one of a pair is a faint body with a
// slightly stronger edge, the active one a brighter body with a solid edge.
const IDLE_BODY: f32 = 3.0 / 15.0;
const IDLE_EDGE: f32 = 4.0 / 15.0;
const LIT_BODY: f32 = 8.0 / 15.0;
const SOLID: f32 = 1.0;

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

/// Both bumpers side by side with one lit, the way the retail glyphs show the
/// two triggers and the two sticks. Monochrome, so it takes the text colour.
fn shoulder_pair(h: usize, lit: Side, labels: [&str; 2], trigger: bool) -> Canvas {
    const BW: f32 = 21.0;
    const GAP: f32 = 3.0;
    let bh = (if trigger { 15.0f32 } else { 12.0f32 }).min(h as f32);
    let w = (BW * 2.0 + GAP) as usize;
    let y0 = ((h as f32 - bh) / 2.0).floor();
    let mut c = Canvas::new(w, h);
    for side in [Side::Left, Side::Right] {
        let x0 = if side == Side::Left { 0.0 } else { BW + GAP };
        let (x1, y1) = (x0 + BW, y0 + bh);
        // A bar whose outer shoulder is rounder than its other corners.
        let shoulder = match side {
            Side::Left => round_rect(x0, y0, x1 + 8.0, y1 + 8.0, 6.0),
            Side::Right => round_rect(x0 - 8.0, y0, x1, y1 + 8.0, 6.0),
        };
        let shape = both(shoulder, round_rect(x0, y0, x1, y1, 2.0));
        let (body, edge) = if side == lit {
            (LIT_BODY, SOLID)
        } else {
            (IDLE_BODY, IDLE_EDGE)
        };
        c.fill_flat(&shape, white(body));
        c.fill_flat(inner_outline(&shape, 1.0), white(edge));
        let text = labels[usize::from(side == Side::Right)];
        let tx = x0 as i32 + (BW as i32 - label::width(text)) / 2;
        let ty = y0 as i32 + (bh as i32 - label::GLYPH_H) / 2;
        label::draw(text, tx, ty, |x, y| c.plot(x, y, white(edge)));
    }
    c
}

// The key and mouse pictures are shaded like the retail White button: a
// light body, brighter at the top, inside a dark rim.
const KEY_TOP: [f32; 3] = [0.97, 0.98, 1.0];
const KEY_BOTTOM: [f32; 3] = [0.66, 0.70, 0.78];
const KEY_LIP: [f32; 3] = [0.42, 0.46, 0.55];
const KEY_RIM: Rgba = [0.16, 0.19, 0.27, 1.0];
const KEY_INK: Rgba = [0.07, 0.09, 0.15, 1.0];
/// The lit part of a mouse picture.
const AMBER_TOP: [f32; 3] = [1.0, 0.78, 0.25];
const AMBER_BOTTOM: [f32; 3] = [0.95, 0.48, 0.05];

fn opaque(rgb: [f32; 3]) -> Rgba {
    [rgb[0], rgb[1], rgb[2], 1.0]
}

/// A paint that runs from `top` at `y0` to `bottom` at `y1`.
fn gradient(top: [f32; 3], bottom: [f32; 3], y0: f32, y1: f32) -> impl Fn(f32, f32) -> Rgba {
    move |_, y| {
        let t = ((y - y0) / (y1 - y0).max(1.0)).clamp(0.0, 1.0);
        [
            top[0] + (bottom[0] - top[0]) * t,
            top[1] + (bottom[1] - top[1]) * t,
            top[2] + (bottom[2] - top[2]) * t,
            1.0,
        ]
    }
}

/// A key with its label: a shaded cap with a darker lip along the bottom.
fn keycap(h: usize, text: &str) -> Canvas {
    let cap_h = if h >= 16 { h - 2 } else { h };
    let tw = label::width(text) as usize;
    let w = (tw + 8).max(cap_h);
    let (y0, y1) = ((h - cap_h) as f32, h as f32);
    let wf = w as f32;
    let shape = round_rect(0.0, y0, wf, y1, 3.0);
    let lip = cap_h >= 14;
    let mut c = Canvas::new(w, h);
    c.fill(&shape, gradient(KEY_TOP, KEY_BOTTOM, y0, y1));
    if lip {
        c.fill_flat(
            both(&shape, round_rect(0.0, y1 - 3.0, wf, y1, 0.0)),
            opaque(KEY_LIP),
        );
    }
    c.fill_flat(inner_outline(&shape, 1.0), KEY_RIM);
    let tx = (w as i32 - tw as i32) / 2;
    let ty = y0 as i32 + (cap_h as i32 - label::GLYPH_H) / 2 - i32::from(lip);
    label::draw(text, tx, ty, |x, y| c.plot(x, y, KEY_INK));
    c
}

/// A mouse seen from above, shaded like a key, with one part lit in amber.
fn mouse(h: usize, lit: MouseIcon) -> Canvas {
    let side = matches!(lit, MouseIcon::Back | MouseIcon::Forward);
    let w = match lit {
        MouseIcon::Move => 21,
        _ if side => 15,
        _ => 13,
    };
    let (bw, bh) = (11.0, (h.saturating_sub(2)).clamp(9, 16) as f32);
    let x0 = if side { 3.0 } else { ((w as f32) - bw) / 2.0 };
    let y0 = ((h as f32 - bh) / 2.0).floor();
    let (x1, y1, xc) = (x0 + bw, y0 + bh, x0 + bw / 2.0);
    let split = (y0 + bh * 0.45).floor() + 0.5;
    let body = round_rect(x0, y0, x1, y1, 5.0);
    let amber = gradient(AMBER_TOP, AMBER_BOTTOM, y0, y1);
    let mut c = Canvas::new(w, h);
    c.fill(&body, gradient(KEY_TOP, KEY_BOTTOM, y0, y1));
    let half = |xa: f32, xb: f32| both(&body, round_rect(xa, y0 - 1.0, xb, split, 0.0));
    match lit {
        MouseIcon::Left => c.fill(half(x0, xc), &amber),
        MouseIcon::Right => c.fill(half(xc, x1), &amber),
        MouseIcon::Middle => {}
        MouseIcon::Forward => {
            c.fill(
                round_rect(x0 - 2.5, split - 4.0, x0 + 1.0, split - 0.5, 1.0),
                &amber,
            );
        }
        MouseIcon::Back => {
            c.fill(
                round_rect(x0 - 2.5, split + 0.5, x0 + 1.0, split + 4.0, 1.0),
                &amber,
            );
        }
        MouseIcon::Move => {
            let m = h as f32 / 2.0;
            c.fill(polygon([(0.5, m), (3.5, m - 3.0), (3.5, m + 3.0)]), &amber);
            let r = w as f32;
            c.fill(
                polygon([(r - 0.5, m), (r - 3.5, m - 3.0), (r - 3.5, m + 3.0)]),
                &amber,
            );
        }
    }
    c.fill_flat(inner_outline(&body, 1.0), KEY_RIM);
    c.fill_flat(segment((x0 + 1.0, split), (x1 - 1.0, split), 1.0), KEY_RIM);
    if lit == MouseIcon::Middle {
        let wheel = round_rect(xc - 2.0, y0 + 1.5, xc + 2.0, split - 0.5, 2.0);
        c.fill(&wheel, &amber);
        c.fill_flat(outline(&wheel, 1.0), KEY_RIM);
    } else {
        c.fill_flat(segment((xc, y0 + 1.0), (xc, split), 1.0), KEY_RIM);
    }
    c
}

#[derive(Clone, Copy)]
enum Symbol {
    Cross,
    Circle,
    Square,
    Triangle,
    Options,
    Share,
}

/// A PlayStation face button in the manner of the retail A, B, X and Y: a
/// shaded ball in the button's colour with a darker rim and a gloss, 16
/// pixels wide, its symbol in a light tint. Options and Share are dark balls
/// with a white symbol, as the retail Start and Back are.
fn face(h: usize, symbol: Symbol) -> Canvas {
    const W: usize = 16;
    let (cx, cy) = (
        W as f32 / 2.0,
        h as f32 / 2.0 + if h >= 18 { 1.0 } else { 0.0 },
    );
    let r = W.min(h) as f32 / 2.0 - 0.5;
    let base: [f32; 3] = match symbol {
        Symbol::Cross => [0.22, 0.42, 0.90],
        Symbol::Circle => [0.84, 0.19, 0.19],
        Symbol::Square => [0.80, 0.34, 0.68],
        Symbol::Triangle => [0.12, 0.62, 0.44],
        Symbol::Options | Symbol::Share => [0.20, 0.20, 0.22],
    };
    let ball = circle(cx, cy, r);
    let mut c = Canvas::new(W, h);
    // A bevel and a small upper-left highlight, rather than a broad gloss
    // stripe. All geometry and lighting are authored here; no sampled pixels.
    c.fill(&ball, |x, y| {
        let (nx, ny) = ((x - cx) / r, (y - cy) / r);
        let z = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
        let diffuse = (-0.35 * nx - 0.50 * ny + 0.78 * z).max(0.0);
        let light = 0.36 + 0.76 * diffuse;
        let shine = ((-(nx + 0.32).powi(2) - (ny + 0.48).powi(2)) / 0.055).exp() * 0.38;
        opaque(base.map(|v| (v * light + shine).min(1.0)))
    });
    c.fill(inner_outline(&ball, 0.85), |x, y| {
        let light = (0.50 - 0.32 * ((x - cx) + (y - cy)) / r).clamp(0.18, 0.88);
        [light, light, light, 0.75]
    });

    let s = r * 0.5;
    // Nearly white, leaning to the ball's colour, like the retail letters.
    let ink = opaque(base.map(|v| 0.86 + 0.14 * v));
    match symbol {
        Symbol::Cross => {
            c.fill_flat(segment((cx - s, cy - s), (cx + s, cy + s), 1.6), ink);
            c.fill_flat(segment((cx - s, cy + s), (cx + s, cy - s), 1.6), ink);
        }
        Symbol::Circle => c.fill_flat(outline(circle(cx, cy, s * 1.15), 1.5), ink),
        Symbol::Square => {
            c.fill_flat(
                outline(round_rect(cx - s, cy - s, cx + s, cy + s, 0.3), 1.5),
                ink,
            );
        }
        Symbol::Triangle => c.fill_flat(
            outline(
                polygon([
                    (cx, cy - s * 1.3),
                    (cx + s * 1.25, cy + s * 0.85),
                    (cx - s * 1.25, cy + s * 0.85),
                ]),
                1.5,
            ),
            ink,
        ),
        Symbol::Options => {
            for dy in [-2.5, 0.0, 2.5] {
                c.fill_flat(segment((cx - 3.0, cy + dy), (cx + 3.0, cy + dy), 1.2), ink);
            }
        }
        Symbol::Share => {
            let (hub, up, down) = ((cx - 2.5, cy), (cx + 2.5, cy - 3.0), (cx + 2.5, cy + 3.0));
            c.fill_flat(segment(hub, up, 1.0), ink);
            c.fill_flat(segment(hub, down, 1.0), ink);
            for p in [hub, up, down] {
                c.fill_flat(circle(p.0, p.1, 1.4), ink);
            }
        }
    }
    c
}

#[cfg(test)]
mod tests;
