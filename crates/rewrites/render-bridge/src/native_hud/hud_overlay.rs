//! Reads the final widget inputs and image at the shader quad before Begin.

use engine::memory::GuestMemory;

/// Material inputs at the completed shader quad, after the guest has selected
/// the widget image and evaluated its (possibly animated) shader parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudTagQuad {
    pub bitmap: u32,
    pub image: u16,
    pub shader: u32,
    pub hud_input: [f32; 4],
    /// A HUD bitmap widget's quad ([`tag_quad`]), not a menu shader
    /// bitmap's ([`menu_shader_quad`]) or a text widget's.
    pub widget: bool,
}

pub const MATERIAL_INPUTS: u32 = 0x004e_6980;

/// The values installed by the guest for the current widget layer.
pub fn inputs(mem: &dyn GuestMemory) -> Option<[f32; 4]> {
    four(mem, MATERIAL_INPUTS)
}

/// The menu shader bitmap's quad call returns here: its draw `0x002b11c9`
/// calls `0x0022a664` at `0x002b12ae` with the definition's bitmap
/// (`+0x10`), image (`+0x0a`) and shader (`+0x18`) (Code, `h2 disasm`).
pub const MENU_SHADER_RETURN: u32 = 0x002b_12b3;
/// The UI's globals: before the quad, `0x002b123e` copies the widget's
/// four shader inputs (`+0x74`) through `0x001392f8` to `+0x1c0` of the
/// block this points at.
pub const UI_GLOBALS: u32 = 0x0051_0c4c;
pub const UI_INPUTS: u32 = 0x1c0;

/// The four values at `at`.
fn four(mem: &dyn GuestMemory, at: u32) -> Option<[f32; 4]> {
    let mut v = [0.0; 4];
    for (i, value) in v.iter_mut().enumerate() {
        *value = f32::from_bits(mem.u32(at.checked_add(4 * i as u32)?).ok()?);
    }
    Some(v)
}

/// The UI's shader inputs as the menu shader bitmap's draw left them.
pub fn ui_inputs(mem: &dyn GuestMemory) -> Option<[f32; 4]> {
    let globals = mem.u32(UI_GLOBALS).ok()?;
    if globals == 0 {
        return None;
    }
    four(mem, globals.checked_add(UI_INPUTS)?)
}

/// The quad call's bitmap, image and shader arguments, when it names both.
fn arguments(mem: &dyn GuestMemory, frame: u32) -> Option<(u32, u16, u32)> {
    let bitmap = mem.u32(frame.checked_add(8)?).ok()?;
    let image = u16::try_from(mem.u32(frame.checked_add(12)?).ok()?).ok()?;
    let shader = mem.u32(frame.checked_add(16)?).ok()?;
    (bitmap != u32::MAX && shader != u32::MAX).then_some((bitmap, image, shader))
}

/// Only the HUD bitmap widget helper's calls ([`menu_shader_quad`] is the
/// menus'); the rest of `0x22a664`'s callers keep the UI fan path.
pub fn tag_quad(mem: &dyn GuestMemory, frame: u32) -> Option<HudTagQuad> {
    let caller = mem.u32(frame.checked_add(4)?).ok()?;
    if !matches!(caller, 0x22b695 | 0x22b561 | 0x22b39c | 0x22b701 | 0x22b754 | 0x22b7d3 | 0x22bca9) {
        return None;
    }
    let (bitmap, image, shader) = arguments(mem, frame)?;
    let hud_input = inputs(mem)?;
    Some(HudTagQuad { bitmap, image, shader, hud_input, widget: true })
}

/// A menu shader bitmap widget's quad (type 9): its definition's bitmap,
/// image and shader, and the UI's inputs in the `hud_input` slot. Its
/// combiner and constants are taken at the draw as a HUD widget's are.
pub fn menu_shader_quad(mem: &dyn GuestMemory, frame: u32) -> Option<HudTagQuad> {
    if mem.u32(frame.checked_add(4)?).ok()? != MENU_SHADER_RETURN {
        return None;
    }
    let (bitmap, image, shader) = arguments(mem, frame)?;
    let hud_input = ui_inputs(mem)?;
    Some(HudTagQuad { bitmap, image, shader, hud_input, widget: false })
}

/// A pointer to the record whose `+0x3fc` is the datum of the `unic` tag
/// the HUD's strings come from (`0x13925f`).
const STRINGS_OWNER: u32 = 0x0051_0c94;
const OWNER_UNIC: u32 = 0x3fc;
/// The tag instance table (`0x10` bytes each, tag data at `+8`) and the
/// language index (`-1` until the lookup first picks one, `0x1a0188`).
const TAG_TABLE: u32 = 0x004e_3b44;
const TAG_STRIDE: u32 = 0x10;
const TAG_DATA: u32 = 8;
const LANGUAGE: u32 = 0x0047_ff38;
/// The globals tag's data, whose locale records are the string tables.
const GLOBALS: u32 = 0x004e_034c;
/// A `unic`'s `(start, count)` pair of `i16`s per language, from its `+0x10`.
const RANGES: u32 = 0x10;
/// The text widget's string ID (`+0x28`, `0x22a97e`).
pub const WIDGET_STRING: u32 = 0x28;
/// The string draw's replacements (`0x22d2ee`): 18 entries, each a wide
/// pattern pointer and the character that takes its place.
const REPLACEMENTS: u32 = 0x0044_a3f0;
const REPLACEMENT_COUNT: u32 = 18;
const REPLACEMENT_STRIDE: u32 = 8;
/// The button glyphs the draw swaps for the player's controls
/// (`0x22d318`..`0x22d32a`).
const SUBSTITUTED: std::ops::RangeInclusive<u16> = 0xe000..=0xf8ff;
/// The lookup's output holds 0xff units (`0x1a0379`).
const MAX_UNITS: usize = 0xff;

/// A text widget's localized string as the game's string tables hold it
/// (`0x22a97b`: `0x13925f` finds the string ID `key` in the language's
/// range of the HUD's `unic`, `0x140440` decodes its UTF-8): `None` when
/// the lookup is not ready (no language chosen, no tables), or when the
/// string draw would change it before drawing, which is not read: button
/// glyphs and the replacement patterns (`0x22d2ee`), a character outside
/// the basic plane or text that is not UTF-8. A string the tables lack is
/// the empty one, as in the game.
pub fn localized(mem: &dyn GuestMemory, key: u32) -> Option<String> {
    use engine::locale::{LocaleTable, StringId};

    if key == 0 {
        return None;
    }
    let owner = mem.u32(STRINGS_OWNER).ok().filter(|&o| o != 0)?;
    let datum = mem.u32(owner.checked_add(OWNER_UNIC)?).ok().filter(|&d| d != u32::MAX)?;
    let language = mem.u32(LANGUAGE).ok().filter(|&l| l != u32::MAX)?;
    let tags = mem.u32(TAG_TABLE).ok()?;
    let instance = tags.checked_add((datum & 0xffff).checked_mul(TAG_STRIDE)?)?;
    let data = mem.u32(instance.checked_add(TAG_DATA)?).ok()?;
    let range = data.checked_add(RANGES)?.checked_add(language.checked_mul(4)?)?;
    let start = i32::from(mem.u16(range).ok()? as i16);
    let count = i32::from(mem.u16(range.checked_add(2)?).ok()? as i16);
    let table = LocaleTable::in_globals(mem.u32(GLOBALS).ok()?, language);
    if !table.is_loaded(mem).ok()? {
        return Some(String::new());
    }
    let at = table.find(mem, start, count, StringId(key)).ok()?;
    let mut bytes = Vec::new();
    for offset in 0..4 * MAX_UNITS as u32 {
        match mem.u8(at.checked_add(offset)?).ok()? {
            0 => break,
            b => bytes.push(b),
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    let units: Vec<u16> = text.chars().map(|c| u16::try_from(u32::from(c)).ok()).collect::<Option<_>>()?;
    if units.iter().any(|u| SUBSTITUTED.contains(u)) {
        return None;
    }
    for entry in 0..REPLACEMENT_COUNT {
        let pattern = mem.u32(REPLACEMENTS + entry * REPLACEMENT_STRIDE).ok()?;
        let mut wide = Vec::new();
        for unit in 0..16 {
            match mem.u16(pattern.checked_add(2 * unit)?).ok()? {
                0 => break,
                u => wide.push(u),
            }
        }
        if !wide.is_empty() && units.windows(wide.len()).any(|w| w == wide) {
            return None;
        }
    }
    Some(String::from_utf16_lossy(&units[..units.len().min(MAX_UNITS)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn shield_layer_reads_its_image_and_evaluated_inputs() {
        let mut m = FakeGuest::new();
        let frame = 0x2000;
        m.put_u32(frame + 4, 0x22b561).put_u32(frame + 8, 0xaa00_0001)
            .put_u32(frame + 12, 1).put_u32(frame + 16, 0xbb00_0002);
        for (i, value) in [0.25, 0.5, -1.0, 2.0].into_iter().enumerate() {
            m.put_f32(MATERIAL_INPUTS + 4 * i as u32, value);
        }
        let quad = tag_quad(&m, frame).unwrap();
        assert_eq!((quad.bitmap, quad.image, quad.shader), (0xaa00_0001, 1, 0xbb00_0002));
        assert_eq!(quad.hud_input, [0.25, 0.5, -1.0, 2.0]);
        assert!(quad.widget);
        m.put_u32(frame + 12, u32::MAX);
        assert!(tag_quad(&m, frame).is_none());
        m.put_u32(frame + 12, 1).put_u32(frame + 4, 0x1234);
        assert!(tag_quad(&m, frame).is_none());
    }
    /// The guest's string lookup over invented tables: the language's
    /// range of the HUD's list, the locale record's entry by full string
    /// ID, its UTF-8 blob.
    fn tables() -> FakeGuest {
        let mut m = FakeGuest::new();
        let (owner, tags, unic, globals, blob) = (0x1000, 0x2000, 0x3000, 0x4000, 0x6000);
        m.put_u32(STRINGS_OWNER, owner).put_u32(owner + OWNER_UNIC, 0x8001_0002);
        m.put_u32(LANGUAGE, 1).put_u32(TAG_TABLE, tags).put_u32(GLOBALS, globals);
        m.put_u32(tags + 2 * TAG_STRIDE + TAG_DATA, unic);
        // Language 1 owns entries 1 to 2.
        m.put_u16(unic + RANGES + 4, 1).put_u16(unic + RANGES + 6, 2);
        // The locale record of language 1: three entries.
        let record = globals + 0x188 + 0x1c;
        let index = 0x5000;
        m.put_u32(record, index).put_u32(record + 4, blob).put_u32(record + 8, 3).put_u8(record + 0x18, 1);
        for (n, (id, offset)) in [(0x0300_0001, 0), (0x0500_0abc, 8), (0x0400_0def, 16)].into_iter().enumerate() {
            m.put_u32(index + 8 * n as u32, id).put_u32(index + 8 * n as u32 + 4, offset);
        }
        m.put(blob, b"zero\0").put(blob + 8, b"Ammo\0").put(blob + 16, "Zur\u{fc}ck\0".as_bytes());
        m.put_u8(engine::locale::EMPTY_STRING, 0);
        // A replacement table whose only pattern is "[x]".
        m.put_u32(REPLACEMENTS, 0x7000).put(0x7000, &[b'[', 0, b'x', 0, b']', 0, 0, 0]);
        for entry in 1..REPLACEMENT_COUNT {
            m.put_u32(REPLACEMENTS + entry * REPLACEMENT_STRIDE, 0x7100);
        }
        m.put_u16(0x7100, 0);
        m
    }

    #[test]
    fn a_localized_string_is_found_in_the_languages_range_by_its_full_id() {
        let mut m = tables();
        assert_eq!(localized(&m, 0x0500_0abc).as_deref(), Some("Ammo"));
        assert_eq!(localized(&m, 0x0400_0def).as_deref(), Some("Zur\u{fc}ck"));
        // Outside the language's range, or a different length byte, it is the empty string.
        assert_eq!(localized(&m, 0x0300_0001).as_deref(), Some(""));
        assert_eq!(localized(&m, 0x0600_0abc).as_deref(), Some(""));
        // Not ready: no language yet, no list, no string.
        assert_eq!(localized(&m, 0), None);
        m.put_u32(LANGUAGE, u32::MAX);
        assert_eq!(localized(&m, 0x0500_0abc), None);
        m.put_u32(LANGUAGE, 1).put_u32(0x1000 + OWNER_UNIC, u32::MAX);
        assert_eq!(localized(&m, 0x0500_0abc), None);
        // A table not loaded draws as the game's empty buffer.
        let mut m = tables();
        m.put_u8(0x4000 + 0x188 + 0x1c + 0x18, 0);
        assert_eq!(localized(&m, 0x0500_0abc).as_deref(), Some(""));
    }

    #[test]
    fn text_the_string_draw_would_change_is_not_read() {
        let mut m = tables();
        // A button glyph, a pattern the draw replaces, a character past the basic plane.
        m.put(0x6008, "A\u{e001}\0".as_bytes());
        assert_eq!(localized(&m, 0x0500_0abc), None);
        m.put(0x6008, b"a[x]b\0");
        assert_eq!(localized(&m, 0x0500_0abc), None);
        m.put(0x6008, "\u{1f600}\0".as_bytes());
        assert_eq!(localized(&m, 0x0500_0abc), None);
        m.put(0x6008, &[0xff, 0]);
        assert_eq!(localized(&m, 0x0500_0abc), None);
        m.put(0x6008, b"[x\0");
        assert_eq!(localized(&m, 0x0500_0abc).as_deref(), Some("[x"));
    }

    /// A menu shader bitmap's quad names its definition's bitmap, image and
    /// shader, and carries the UI's inputs, not the HUD's.
    #[test]
    fn a_menu_shader_bitmap_reads_its_arguments_and_the_uis_inputs() {
        let mut m = FakeGuest::new();
        let (frame, globals) = (0x2000, 0x7000);
        m.put_u32(frame + 4, MENU_SHADER_RETURN).put_u32(frame + 8, 0xaa00_0003)
            .put_u32(frame + 12, 2).put_u32(frame + 16, 0xbb00_0004)
            .put_u32(UI_GLOBALS, globals);
        for (i, value) in [0.75, 0.0, 1.0, -0.5].into_iter().enumerate() {
            m.put_f32(globals + UI_INPUTS + 4 * i as u32, value);
            m.put_f32(MATERIAL_INPUTS + 4 * i as u32, 9.0);
        }
        let quad = menu_shader_quad(&m, frame).unwrap();
        assert_eq!((quad.bitmap, quad.image, quad.shader), (0xaa00_0003, 2, 0xbb00_0004));
        assert_eq!(quad.hud_input, [0.75, 0.0, 1.0, -0.5]);
        assert!(!quad.widget);
        assert!(tag_quad(&m, frame).is_none(), "not a HUD widget's call");
        m.put_u32(frame + 16, u32::MAX);
        assert!(menu_shader_quad(&m, frame).is_none(), "no shader");
        m.put_u32(frame + 16, 0xbb00_0004).put_u32(UI_GLOBALS, 0);
        assert!(menu_shader_quad(&m, frame).is_none(), "no UI globals");
    }
}
