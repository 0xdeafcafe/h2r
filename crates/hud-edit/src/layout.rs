//! The `nhdt` (new HUD definition) as the engine reads it: the root's
//! widget blocks, the bitmap and text widgets' bytes, and the field table
//! the editor's property panel is drawn from.
//!
//! Every offset here is read by the engine code cited (E: an address in
//! the retail `default.xbe`, read with `h2 disasm`). The same layout is
//! written by the Halo 1 converter (`h1objects::health_hud::widget`,
//! `h1objects::h1hud::text`) and documented in docs/halo1/hud.md and
//! docs/halo1/health.md; docs/editor/hud-editor.md gathers it.
//!
//! [`BITMAP_FIELDS`] and [`TEXT_FIELDS`] are plain data on purpose: they
//! are the seam a generic labelled tag field panel can take over (each row
//! is a label, an offset in the widget, a type and its proof), with
//! [`crate::model::Widget::get`] and [`crate::model::Widget::set`] as the
//! value accessors.

/// The `nhdt` root: `0x28` bytes (E `0x13a532` walks the blocks below).
pub const ROOT_SIZE: usize = 0x28;
/// Bitmap widgets: a block (count, address) at root `+0x08`, `0x64` each
/// (E `0x13a532`: `+0x08` count, `+0x0c` address).
pub const ROOT_BITMAPS: usize = 0x08;
pub const BITMAP_SIZE: usize = 0x64;
/// Text widgets: root `+0x10`, `0x54` each (E `0x13a532`, the text pass
/// calling `0x22aa16`).
pub const ROOT_TEXTS: usize = 0x10;
pub const TEXT_SIZE: usize = 0x54;

/// The three screen modes a widget carries a placement for: fullscreen,
/// half and quarter screen (split screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Full = 0,
    Half = 1,
    Quarter = 2,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Full, Mode::Half, Mode::Quarter];
    pub fn label(self) -> &'static str {
        match self {
            Mode::Full => "fullscreen",
            Mode::Half => "half screen",
            Mode::Quarter => "quarter screen",
        }
    }
}

/// Bitmap widget fields (E: the gate `0x13ac87`, the inputs `0x13ad48`,
/// the draw `0x22aff1` and its sprite pick `0x22ae21`).
pub mod bitmap {
    /// Name (string ID; debug only).
    pub const NAME: usize = 0x00;
    /// Four input bytes (`0x13ad48` turns each into a float; the draw
    /// copies them to the shader's `hud_input1`..`4`, `0x22b181`).
    pub const INPUTS: usize = 0x04;
    /// State flags (`0x13ac87`, from `+0x08`): see [`crate::gate`].
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    /// Anchor (`u16`; the corner `0x1396c7` returns).
    pub const ANCHOR: usize = 0x1c;
    /// Flags (`u16`; bits 0 and 1 mirror the texture, `0x22b29c`).
    pub const FLAGS: usize = 0x1e;
    /// Bitmap and shader (8-byte tag references: group, datum).
    pub const BITMAP: usize = 0x20;
    pub const SHADER: usize = 0x28;
    /// Sequence (`i8`) per screen mode (`0x22ae7f`, `0x22ae79`, `0x22ae73`).
    pub const SEQUENCES: usize = 0x30;
    /// Offset (two `i16`, x then y) per screen mode (`0x22b128`,
    /// `0x22b0da`, `0x22b089`).
    pub const OFFSETS: [usize; 3] = [0x34, 0x38, 0x3c];
    /// Registration (two `f32`, fractions of the sprite) per screen mode
    /// (`0x22b156`, `0x22b108`, `0x22b0b7`).
    pub const REGISTRATIONS: [usize; 3] = [0x40, 0x48, 0x50];
    /// The widget's effects block (count, address), handed to `0x22bcaf`
    /// before the draw (`0x22b191`); not modelled by the preview.
    pub const EFFECTS: usize = 0x58;
    /// Special type (`u16`, `0x22b2d0`): 5 is the shield meter.
    pub const SPECIAL: usize = 0x60;
}

/// Text widget fields (E: the draw `0x22aa16`, its string `0x22a871`).
pub mod text {
    pub const NAME: usize = 0x00;
    pub const INPUTS: usize = 0x04;
    pub const STATE: usize = 0x08;
    pub const STATE_END: usize = 0x1c;
    pub const ANCHOR: usize = 0x1c;
    /// Flags (`u16`): 1 draws the number [`STRING`] names; 2 pads it to two
    /// digits, 4 to three; 8 draws the player's name.
    pub const FLAGS: usize = 0x1e;
    /// Shader (tag reference; no shader, no draw: `0x22aa2f`).
    pub const SHADER: usize = 0x20;
    /// String ID (no string, no draw: `0x22aa26`).
    pub const STRING: usize = 0x28;
    /// Justification (`u16`, `0x22ac0b`): 0 starts at the point, 1 is
    /// centred on it, 2 ends at it.
    pub const JUSTIFICATION: usize = 0x2c;
    /// Font (`i8`) per screen mode (`0x22a9ae`, `0x22a9a9`, `0x22a9a4`):
    /// 0 is engine font 6, 1 engine font 8, anything else no font
    /// (`0x22a85b`).
    pub const FONTS: usize = 0x30;
    /// Offset (two `i16`) per screen mode (`0x22ab46`, `0x22ab38`,
    /// `0x22ab2a`).
    pub const OFFSETS: [usize; 3] = [0x40, 0x44, 0x48];
    /// Effects block (count, address), handed to `0x22bcaf` (`0x22ab72`).
    pub const EFFECTS: usize = 0x4c;
}

/// The engine font a text widget's font byte names (E `0x22a85b`).
pub fn text_font(byte: i8) -> Option<i16> {
    match byte {
        0 => Some(6),
        1 => Some(8),
        _ => None,
    }
}

/// The special type `0x22aff1` draws as the shield meter.
pub const SPECIAL_SHIELD: u16 = 5;

/// A field's type, as the property panel edits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    I8,
    U8,
    I16,
    U16,
    /// A `u16` shown in hex (flag words).
    Hex16,
    F32,
    /// An 8-byte tag reference (group, datum); the datum is edited.
    TagRef,
    /// A string ID, shown by name, not edited here.
    StringId,
}

impl Kind {
    pub fn size(self) -> usize {
        match self {
            Kind::I8 | Kind::U8 => 1,
            Kind::I16 | Kind::U16 | Kind::Hex16 => 2,
            Kind::F32 | Kind::StringId => 4,
            Kind::TagRef => 8,
        }
    }
}

/// One editable field of a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    /// Stable key (also the edit's name in the undo history).
    pub key: &'static str,
    pub label: &'static str,
    /// Byte offset in the widget.
    pub offset: usize,
    pub kind: Kind,
    /// The panel's section.
    pub group: &'static str,
    /// The screen mode the field belongs to, when it is one of three.
    pub mode: Option<Mode>,
    /// Where the meaning comes from (E address, or a note).
    pub proof: &'static str,
}

const fn f(
    key: &'static str,
    label: &'static str,
    offset: usize,
    kind: Kind,
    group: &'static str,
    mode: Option<Mode>,
    proof: &'static str,
) -> Field {
    Field {
        key,
        label,
        offset,
        kind,
        group,
        mode,
        proof,
    }
}

use Kind::*;
use Mode::*;

/// A bitmap widget's fields, in the panel's order.
pub const BITMAP_FIELDS: &[Field] = &[
    f("name", "Name", 0x00, StringId, "Widget", None, "debug only"),
    f("anchor", "Anchor", 0x1c, U16, "Widget", None, "E 0x1396c7"),
    f("flags", "Flags", 0x1e, Hex16, "Widget", None, "E 0x22b29c: bits 0, 1 mirror"),
    f("special", "Special type", 0x60, U16, "Widget", None, "E 0x22b2d0: 5 shield meter"),
    f("bitmap", "Bitmap", 0x20, TagRef, "Art", None, "E 0x22ae21"),
    f("shader", "Shader", 0x28, TagRef, "Art", None, "E 0x22aff1"),
    f("seq0", "Sequence", 0x30, I8, "Art", Some(Full), "E 0x22ae7f"),
    f("seq1", "Sequence", 0x31, I8, "Art", Some(Half), "E 0x22ae79"),
    f("seq2", "Sequence", 0x32, I8, "Art", Some(Quarter), "E 0x22ae73"),
    f("x0", "Offset x", 0x34, I16, "Placement", Some(Full), "E 0x22b128"),
    f("y0", "Offset y", 0x36, I16, "Placement", Some(Full), "E 0x22b135"),
    f("x1", "Offset x", 0x38, I16, "Placement", Some(Half), "E 0x22b0da"),
    f("y1", "Offset y", 0x3a, I16, "Placement", Some(Half), "E 0x22b0e7"),
    f("x2", "Offset x", 0x3c, I16, "Placement", Some(Quarter), "E 0x22b089"),
    f("y2", "Offset y", 0x3e, I16, "Placement", Some(Quarter), "E 0x22b096"),
    f("rx0", "Registration x", 0x40, F32, "Placement", Some(Full), "E 0x22b156"),
    f("ry0", "Registration y", 0x44, F32, "Placement", Some(Full), "E 0x22b16f"),
    f("rx1", "Registration x", 0x48, F32, "Placement", Some(Half), "E 0x22b108"),
    f("ry1", "Registration y", 0x4c, F32, "Placement", Some(Half), "E 0x22b121"),
    f("rx2", "Registration x", 0x50, F32, "Placement", Some(Quarter), "E 0x22b0b7"),
    f("ry2", "Registration y", 0x54, F32, "Placement", Some(Quarter), "E 0x22b0d0"),
    f("in0", "Input 1", 0x04, U8, "Inputs", None, "E 0x13ad48"),
    f("in1", "Input 2", 0x05, U8, "Inputs", None, "E 0x13ad48"),
    f("in2", "Input 3", 0x06, U8, "Inputs", None, "E 0x13ad48"),
    f("in3", "Input 4", 0x07, U8, "Inputs", None, "E 0x13ad48"),
    f("show_a", "Show when A", 0x08, Hex16, "State", None, "E 0x13acd5"),
    f("show_b", "Show when B", 0x0a, Hex16, "State", None, "E 0x13ace2"),
    f("show_c", "Show when C", 0x0c, Hex16, "State", None, "E 0x13acf0"),
    f("show_d", "Show when D", 0x0e, Hex16, "State", None, "E 0x13acfd"),
    f("hide_a", "Hide when A", 0x10, Hex16, "State", None, "E 0x13ad0b"),
    f("hide_b", "Hide when B", 0x12, Hex16, "State", None, "E 0x13ad19"),
    f("hide_c", "Hide when C", 0x14, Hex16, "State", None, "E 0x13ad27"),
    f("hide_d", "Hide when D", 0x16, Hex16, "State", None, "E 0x13ad32"),
];

/// A text widget's fields, in the panel's order.
pub const TEXT_FIELDS: &[Field] = &[
    f("name", "Name", 0x00, StringId, "Widget", None, "debug only"),
    f("anchor", "Anchor", 0x1c, U16, "Widget", None, "E 0x1396c7 (0x22ab17)"),
    f("flags", "Flags", 0x1e, Hex16, "Widget", None, "E 0x22a8a4: 1 number, 2/4 pad, 8 name"),
    f("string", "String", 0x28, StringId, "Text", None, "E 0x22a871"),
    f("justify", "Justification", 0x2c, U16, "Text", None, "E 0x22ac0b: 0 start, 1 centre, 2 end"),
    f("shader", "Shader", 0x20, TagRef, "Text", None, "E 0x22aa2f"),
    f("font0", "Font", 0x30, I8, "Text", Some(Full), "E 0x22a85b: 0 font 6, 1 font 8"),
    f("font1", "Font", 0x31, I8, "Text", Some(Half), "E 0x22a85b"),
    f("font2", "Font", 0x32, I8, "Text", Some(Quarter), "E 0x22a85b"),
    f("x0", "Offset x", 0x40, I16, "Placement", Some(Full), "E 0x22ab46"),
    f("y0", "Offset y", 0x42, I16, "Placement", Some(Full), "E 0x22ab4e"),
    f("x1", "Offset x", 0x44, I16, "Placement", Some(Half), "E 0x22ab38"),
    f("y1", "Offset y", 0x46, I16, "Placement", Some(Half), "E 0x22ab40"),
    f("x2", "Offset x", 0x48, I16, "Placement", Some(Quarter), "E 0x22ab2a"),
    f("y2", "Offset y", 0x4a, I16, "Placement", Some(Quarter), "E 0x22ab32"),
    f("in0", "Input 1", 0x04, U8, "Inputs", None, "E 0x13ad48"),
    f("in1", "Input 2", 0x05, U8, "Inputs", None, "E 0x13ad48"),
    f("in2", "Input 3", 0x06, U8, "Inputs", None, "E 0x13ad48"),
    f("in3", "Input 4", 0x07, U8, "Inputs", None, "E 0x13ad48"),
    f("show_a", "Show when A", 0x08, Hex16, "State", None, "E 0x13acd5"),
    f("show_b", "Show when B", 0x0a, Hex16, "State", None, "E 0x13ace2"),
    f("show_c", "Show when C", 0x0c, Hex16, "State", None, "E 0x13acf0"),
    f("show_d", "Show when D", 0x0e, Hex16, "State", None, "E 0x13acfd"),
    f("hide_a", "Hide when A", 0x10, Hex16, "State", None, "E 0x13ad0b"),
    f("hide_b", "Hide when B", 0x12, Hex16, "State", None, "E 0x13ad19"),
    f("hide_c", "Hide when C", 0x14, Hex16, "State", None, "E 0x13ad27"),
    f("hide_d", "Hide when D", 0x16, Hex16, "State", None, "E 0x13ad32"),
];

/// The offset field keys (x, y) for a mode.
pub fn offset_keys(mode: Mode) -> (&'static str, &'static str) {
    match mode {
        Mode::Full => ("x0", "y0"),
        Mode::Half => ("x1", "y1"),
        Mode::Quarter => ("x2", "y2"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field lies inside its widget, no two overlap, and the table
    /// agrees with the named offsets above.
    #[test]
    fn the_field_tables_fit_their_widgets() {
        for (fields, size) in [(BITMAP_FIELDS, BITMAP_SIZE), (TEXT_FIELDS, TEXT_SIZE)] {
            let mut used = vec![None; size];
            for fl in fields {
                assert!(fl.offset + fl.kind.size() <= size, "{}", fl.key);
                for b in &mut used[fl.offset..fl.offset + fl.kind.size()] {
                    assert!(b.is_none(), "{} overlaps {:?}", fl.key, b);
                    *b = Some(fl.key);
                }
            }
            let keys: Vec<_> = fields.iter().map(|f| f.key).collect();
            let mut dedup = keys.clone();
            dedup.sort();
            dedup.dedup();
            assert_eq!(dedup.len(), keys.len(), "unique keys");
        }
        let at = |fields: &[Field], k: &str| fields.iter().find(|f| f.key == k).unwrap().offset;
        for m in Mode::ALL {
            let (x, y) = offset_keys(m);
            assert_eq!(at(BITMAP_FIELDS, x), bitmap::OFFSETS[m as usize]);
            assert_eq!(at(BITMAP_FIELDS, y), bitmap::OFFSETS[m as usize] + 2);
            assert_eq!(at(TEXT_FIELDS, x), text::OFFSETS[m as usize]);
            assert_eq!(at(BITMAP_FIELDS, &format!("rx{}", m as usize)), bitmap::REGISTRATIONS[m as usize]);
        }
        assert_eq!(at(BITMAP_FIELDS, "special"), bitmap::SPECIAL);
        assert_eq!(at(TEXT_FIELDS, "justify"), text::JUSTIFICATION);
        assert_eq!(bitmap::EFFECTS + 8, bitmap::SPECIAL);
        assert_eq!(text::EFFECTS + 8, TEXT_SIZE);
    }

    #[test]
    fn the_font_byte_names_engine_fonts_6_and_8() {
        assert_eq!(text_font(0), Some(6));
        assert_eq!(text_font(1), Some(8));
        assert_eq!(text_font(2), None);
        assert_eq!(text_font(-1), None);
    }
}
