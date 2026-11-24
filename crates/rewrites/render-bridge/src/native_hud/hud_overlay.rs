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
    let mut hud_input = [0.0; 4];
    for (i, value) in hud_input.iter_mut().enumerate() {
        *value = f32::from_bits(mem.u32(MATERIAL_INPUTS + 4 * i as u32).ok()?);
    }
    Some(hud_input)
}

/// Only the bitmap widget helper's calls. The screen widgets also use
/// 0x22a664, with their own vertex setup and blend state.
pub fn tag_quad(mem: &dyn GuestMemory, frame: u32) -> Option<HudTagQuad> {
    let caller = mem.u32(frame.checked_add(4)?).ok()?;
    if !matches!(caller, 0x22b695 | 0x22b561 | 0x22b39c | 0x22b701 | 0x22b754 | 0x22b7d3 | 0x22bca9) {
        return None;
    }
    let bitmap = mem.u32(frame.checked_add(8)?).ok()?;
    let image = u16::try_from(mem.u32(frame.checked_add(12)?).ok()?).ok()?;
    let shader = mem.u32(frame.checked_add(16)?).ok()?;
    if bitmap == u32::MAX || shader == u32::MAX {
        return None;
    }
    let hud_input = inputs(mem)?;
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
}
