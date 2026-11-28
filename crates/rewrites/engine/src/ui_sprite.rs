//! Inputs to the loading bitmap fan at `0x0002ae60`. The caller has picked
//! the sampler register and rectangles and already set its shader and target.
use crate::light_constants::Sse;
use guest_layout::memory::GuestMemory;
use crate::screen_grid::constant::ONE;

/// Full integer depth range used for the loading sprites' z and w.
pub const D24_FULL_SCALE: f32 = ((1u32 << 24) - 1) as f32;

type BitmapFan = [[(u8, [f32; 4]); 2]; 4];

/// Build the same four vertices using SSE's live rounding and exception mode.
/// `frame` is the stack after the Begin primitive word is popped. The two
/// rectangle pointers are to float left/right/top/bottom bounds.
pub fn mapped_rectangle(
    mem: &dyn GuestMemory,
    frame: u32,
    position: u32,
    texture: u32,
    register: u8,
    mxcsr: &mut u32,
) -> Option<BitmapFan> {
    if !(1..16).contains(&register) {
        return None;
    }
    let word = |base: u32, offset| mem.u32(base.checked_add(offset)?).ok();
    let bounds = |base| Some([word(base, 0)?, word(base, 4)?, word(base, 8)?, word(base, 12)?]);
    let xy = bounds(position)?;
    let uv = bounds(texture)?;
    let one = mem.u32(ONE).ok()?;
    let mut sse = Sse { mxcsr: *mxcsr };
    let inv_h = sse.div(one, word(frame, 0x14)?).ok()?;
    let inv_w = sse.div(one, word(frame, 0x10)?).ok()?;

    let mut vertices = [[(0, [0.0; 4]); 2]; 4];
    for (vertex, (x, y)) in vertices.iter_mut().zip([(0, 2), (1, 2), (1, 3), (0, 3)]) {
        let u = f32::from_bits(sse.mul(uv[x], inv_w).ok()?);
        let v = f32::from_bits(sse.mul(uv[y], inv_h).ok()?);
        *vertex = [
            (register, [u, v, 0.0, 1.0]),
            (0, [f32::from_bits(xy[x]), f32::from_bits(xy[y]), D24_FULL_SCALE, D24_FULL_SCALE]),
        ];
    }
    *mxcsr = sse.mxcsr;
    Some(vertices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn bitmap_fan_uses_the_live_register_rectangles_and_reciprocals() {
        const FRAME: u32 = 0x1000;
        const POSITION: u32 = 0x2000;
        const TEXTURE: u32 = 0x3000;
        let mut mem = FakeGuest::new();
        for (base, values) in [
            (POSITION, [-3.0f32, 7.0, 2.0, 18.0]),
            (TEXTURE, [4.0, 12.0, 8.0, 32.0]),
        ] {
            for (i, value) in values.into_iter().enumerate() {
                mem.put_f32(base + i as u32 * 4, value);
            }
        }
        mem.put_f32(FRAME + 0x10, 16.0);
        mem.put_f32(FRAME + 0x14, 64.0);
        mem.put_f32(ONE, 1.0);

        let mut mxcsr = 0x1f80;
        let fan = mapped_rectangle(&mem, FRAME, POSITION, TEXTURE, 2, &mut mxcsr).unwrap();
        assert_eq!(fan.map(|v| v[0]), [
            (2, [0.25, 0.125, 0.0, 1.0]),
            (2, [0.75, 0.125, 0.0, 1.0]),
            (2, [0.75, 0.5, 0.0, 1.0]),
            (2, [0.25, 0.5, 0.0, 1.0]),
        ]);
        assert_eq!(fan[2][1], (0, [7.0, 18.0, D24_FULL_SCALE, D24_FULL_SCALE]));
        assert!(mapped_rectangle(&mem, FRAME, POSITION, TEXTURE, 0, &mut mxcsr).is_none());
    }
}
