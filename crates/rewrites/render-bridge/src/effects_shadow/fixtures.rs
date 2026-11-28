//! A particle callback's world for the answer's and the mirror's tests.

use engine::memory::{FlatMemory, GuestMemory};
use engine::particle_emission::constant;

use super::view_update;

pub const ESP: u32 = 0x004f_0000;
const DEF: u32 = 0x004c_0000;
const STATE: u32 = 0x004c_2000;
const DRAW: u32 = 0x004c_4000;
const TINT: u32 = 0x004c_5000;
const BLOB: u32 = 0x004c_6000;
const ONE: u32 = 0x0045_dbc0;

/// A particle whose definition takes its window and corners from itself
/// and every property 0, entered at [`ESP`].
pub fn world() -> FlatMemory {
    let mut m = FlatMemory::new(0x0041_0000, 0x000f_0000);
    m.set_u32(view_update::FRAME, 7).unwrap();
    m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
    m.set_u32(constant::PACK, 255.0f32.to_bits()).unwrap();
    m.set_u32(0x0046_86cc, BLOB).unwrap();
    m.set_u32(DEF, 0x1_0000).unwrap();
    for p in [0x1c, 0x2c, 0x3c, 0x4c] {
        m.set_u32(DEF + p + 8, 1).unwrap();
        m.set_u32(DEF + p + 0xc, BLOB).unwrap();
        m.set_u16(DEF + p + 6, u16::MAX).unwrap();
    }
    m.set_u32(DEF + 0x98, 1).unwrap();
    m.set_u32(DRAW, DEF).unwrap();
    let args = [DEF + 0x200, STATE, 0x004d_0000, 0x004d_0010, 0, TINT, DRAW];
    for (i, a) in args.into_iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, a).unwrap();
    }
    m
}
