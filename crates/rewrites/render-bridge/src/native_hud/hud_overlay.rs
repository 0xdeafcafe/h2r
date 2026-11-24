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
    Some(HudTagQuad { bitmap, image, shader, hud_input })
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
    Some(HudTagQuad { bitmap, image, shader, hud_input })
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
        m.put_u32(frame + 12, u32::MAX);
        assert!(tag_quad(&m, frame).is_none());
        m.put_u32(frame + 12, 1).put_u32(frame + 4, 0x1234);
        assert!(tag_quad(&m, frame).is_none());
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
        assert!(tag_quad(&m, frame).is_none(), "not a HUD widget's call");
        m.put_u32(frame + 16, u32::MAX);
        assert!(menu_shader_quad(&m, frame).is_none(), "no shader");
        m.put_u32(frame + 16, 0xbb00_0004).put_u32(UI_GLOBALS, 0);
        assert!(menu_shader_quad(&m, frame).is_none(), "no UI globals");
    }
}
