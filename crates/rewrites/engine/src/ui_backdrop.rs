//! Immediate geometry of the HUD motion sensor backdrop. Texture selection, shader setup
//! and render targets belong to the caller of each pass.
use guest_layout::memory::GuestMemory;
use crate::{light_constants::Sse, ui_sprite::D24_FULL_SCALE};

pub type Fan<const N: usize> = [[(u8, [f32; 4]); N]; 4];

const SAMPLE_HALF: u32 = 0x0045_dbbc;
const WASH_ALPHA: u32 = 0x0045_dbc0;
const WASH_WIDTH: u32 = 0x0045_dc90;
const WASH_FIRST_DEPTH: u32 = 0x0045_dd40;
// Immediate operands in the lifted passes. Read them from the active title
// so the native branch uses the same values as the guest branch.
const WASH_CORNER_EXTENT: [u32; 3] = [0x0025_4d8f, 0x0025_4d8a, 0x0025_4dae];
const TINT_CORNER_EXTENT: [u32; 4] = [0x0025_4e7f, 0x0025_4ea7, 0x0025_4ea2, 0x0025_4ece];
const TINT_RGBA: [u32; 4] = [0x0025_4e3d, 0x0025_4e38, 0x0025_4e33, 0x0025_4e2e];
const WASH_PARAMETER: u32 = 0x28;
const COMPOSITE_CENTER: u32 = 0x20;
const COMPOSITE_HALF_EXTENT: u32 = 0x24;

fn read(mem: &dyn GuestMemory, base: u32, offset: u32) -> Option<u32> {
    mem.u32(base.checked_add(offset)?).ok()
}

fn uv(u: f32, v: f32) -> (u8, [f32; 4]) {
    (1, [u, v, 0.0, 1.0])
}

fn pos(x: f32, y: f32) -> (u8, [f32; 4]) {
    (0, [x, y, D24_FULL_SCALE, D24_FULL_SCALE])
}

/// The raw fan's maximum push size, including its single colour write.
pub const WASH_PUSH_BYTES: u32 = (2 + 5 + 4 * (3 + 5) + 2) * 4;

/// The intermediate wash. The first two positions use the live wash depth
/// and a different width from the last two positions.
pub fn wash(
    mem: &dyn GuestMemory,
    frame: u32,
    rgb: u32,
    mxcsr: &mut u32,
) -> Option<Fan<3>> {
    if rgb == 0 {
        return None;
    }
    let color = [
        read(mem, rgb, 0)?, read(mem, rgb, 4)?, read(mem, rgb, 8)?,
        read(mem, WASH_ALPHA, 0)?,
    ].map(f32::from_bits);
    let half = read(mem, SAMPLE_HALF, 0)?;
    let width = f32::from_bits(read(mem, WASH_WIDTH, 0)?);
    let [right, bottom_right, bottom_left] = WASH_CORNER_EXTENT;
    let [right, bottom_right, bottom_left] = [
        read(mem, right, 0)?, read(mem, bottom_right, 0)?, read(mem, bottom_left, 0)?,
    ].map(f32::from_bits);
    let depth = f32::from_bits(read(mem, WASH_FIRST_DEPTH, 0)?);
    let mut sse = Sse { mxcsr: *mxcsr };
    let sample = sse.mul(read(mem, frame, WASH_PARAMETER)?, half).ok()?;
    let hi = f32::from_bits(sse.add(sample, half).ok()?);
    let lo = f32::from_bits(sse.sub(half, sample).ok()?);
    let tint = (5, color);
    *mxcsr = sse.mxcsr;
    // The colour is constant across the primitive and is the final carry.
    Some([
        [tint, uv(hi, lo), (0, [0.0, 0.0, depth, depth])],
        [tint, uv(lo, lo), (0, [width, 0.0, depth, depth])],
        [tint, uv(hi, lo), pos(right, bottom_right)],
        [tint, uv(hi, hi), pos(0.0, bottom_left)],
    ])
}

/// The second intermediate fan uses the opposite horizontal UV order from
/// the composite. Its RGB(A) remains in register 5 for the next draw.
pub fn tint(mem: &dyn GuestMemory) -> Option<Fan<3>> {
    let [r, g, b, a] = TINT_RGBA;
    let color = (5, [
        read(mem, r, 0)?, read(mem, g, 0)?, read(mem, b, 0)?, read(mem, a, 0)?,
    ].map(f32::from_bits));
    let [top_x, bottom_x, right_y, left_y] = TINT_CORNER_EXTENT;
    let [top_x, bottom_x, right_y, left_y] = [
        read(mem, top_x, 0)?, read(mem, bottom_x, 0)?,
        read(mem, right_y, 0)?, read(mem, left_y, 0)?,
    ].map(f32::from_bits);
    Some([
        [color, uv(1.0, 0.0), pos(0.0, 0.0)],
        [color, uv(0.0, 0.0), pos(top_x, 0.0)],
        [color, uv(0.0, 1.0), pos(bottom_x, right_y)],
        [color, uv(1.0, 1.0), pos(0.0, left_y)],
    ])
}

/// Composite on the output target using the caller's center and selected
/// half extent. Evaluate the two operations in vertex issue order.
pub fn composite(mem: &dyn GuestMemory, frame: u32, mxcsr: &mut u32) -> Option<Fan<2>> {
    let center = read(mem, frame, COMPOSITE_CENTER)?;
    if center == 0 {
        return None;
    }
    let x = read(mem, center, 0)?;
    let y = read(mem, center, 4)?;
    let half = read(mem, frame, COMPOSITE_HALF_EXTENT)?;
    let mut sse = Sse { mxcsr: *mxcsr };
    let mut fan = [[uv(0.0, 0.0); 2]; 4];
    for (vertex, [u, v]) in fan.iter_mut().zip([[0u8, 0u8], [1, 0], [1, 1], [0, 1]]) {
        let py = if v == 0 { sse.sub(y, half) } else { sse.add(y, half) }.ok()?;
        let px = if u == 0 { sse.sub(x, half) } else { sse.add(x, half) }.ok()?;
        *vertex = [uv(f32::from(u), f32::from(v)), pos(f32::from_bits(px), f32::from_bits(py))];
    }
    *mxcsr = sse.mxcsr;
    Some(fan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn wash_uses_the_live_colour_depth_and_parameter() {
        let frame = 0x1000;
        let rgb = 0x2000;
        let mut mem = FakeGuest::new();
        mem.put_f32(rgb, 0.125)
            .put_f32(rgb + 4, 0.375)
            .put_f32(rgb + 8, 0.75)
            .put_f32(WASH_ALPHA, 0.5)
            .put_f32(SAMPLE_HALF, 0.5)
            .put_f32(WASH_WIDTH, 23.0)
            .put_f32(WASH_FIRST_DEPTH, 17.0)
            .put_f32(frame + WASH_PARAMETER, 0.5);
        for (addr, value) in WASH_CORNER_EXTENT.into_iter().zip([11.0, 12.0, 13.0]) {
            mem.put_f32(addr, value);
        }
        let fan = wash(&mem, frame, rgb, &mut 0x1f80).unwrap();
        assert_eq!(fan[0][0], (5, [0.125, 0.375, 0.75, 0.5]));
        assert_eq!(fan[1][1].1[..2], [0.25, 0.25]);
        assert_eq!(fan[3][1].1[..2], [0.75, 0.75]);
        assert_eq!(fan[1][2], (0, [23.0, 0.0, 17.0, 17.0]));
        assert_eq!(fan[2][2], pos(11.0, 12.0));
    }

    #[test]
    fn tint_flips_u_and_composite_preserves_the_center() {
        let mut mem = FakeGuest::new();
        for (address, color) in TINT_RGBA.into_iter().zip([0.25, 0.5, 0.875, 1.0]) {
            mem.put_f32(address, color);
        }
        for (addr, value) in TINT_CORNER_EXTENT.into_iter().zip([12.0, 13.0, 14.0, 15.0]) {
            mem.put_f32(addr, value);
        }
        mem.put_u32(0x1000 + COMPOSITE_CENTER, 0x2000)
            .put_f32(0x1000 + COMPOSITE_HALF_EXTENT, 3.0)
            .put_f32(0x2000, 7.0)
            .put_f32(0x2004, 19.0);
        let tint = tint(&mem).unwrap();
        assert_eq!(tint[0][0], (5, [0.25, 0.5, 0.875, 1.0]));
        assert_eq!(tint[0][1], uv(1.0, 0.0));
        assert_eq!(tint[2][1], uv(0.0, 1.0));
        assert_eq!(tint[2][2], pos(13.0, 14.0));
        let composite = composite(&mem, 0x1000, &mut 0x1f80).unwrap();
        assert_eq!(composite[0], [uv(0.0, 0.0), pos(4.0, 16.0)]);
        assert_eq!(composite[2], [uv(1.0, 1.0), pos(10.0, 22.0)]);
        assert!(super::composite(&mem, 0x3000, &mut 0x1f80).is_none());
    }
}
