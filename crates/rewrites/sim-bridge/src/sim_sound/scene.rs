//! The mixer feed's scene: the first listener, read from the sound
//! globals once a frame (docs/engine/sound-instances.md, "Mixer feed").
//! Where each sound plays comes from the table
//! ([`sim_sound::instances::Sounds::sources`]), not guest memory.

use guest_layout::points;
use guest_layout::{GuestMemory, MemFault};
use sim_sound::feed::Ear;

/// Sound globals `+0x88`: a user's listener, `0x48` bytes each.
pub const LISTENERS: u32 = 0x88;
pub const LISTENER: u32 = 0x48;
/// The listener's active byte and its frame: forward, left, up, position.
pub const ACTIVE: u32 = 6;
pub const FORWARD: u32 = 0xc;
pub const LEFT: u32 = 0x18;
pub const UP: u32 = 0x24;
pub const POSITION: u32 = 0x30;

fn vector(m: &dyn GuestMemory, at: u32) -> Result<[f32; 3], MemFault> {
    Ok([m.f32(at)?, m.f32(at.wrapping_add(4))?, m.f32(at.wrapping_add(8))?])
}

/// The first active listener of the four users, `None` with no sound
/// globals or none active.
pub fn ear(m: &dyn GuestMemory) -> Result<Option<Ear>, MemFault> {
    let globals = m.u32(points::SOUND_GLOBALS.base)?;
    if globals == 0 {
        return Ok(None);
    }
    for user in 0..4u32 {
        let l = globals.wrapping_add(LISTENERS).wrapping_add(LISTENER * user);
        if m.u8(l.wrapping_add(ACTIVE))? == 0 {
            continue;
        }
        return Ok(Some(Ear {
            position: vector(m, l.wrapping_add(POSITION))?,
            forward: vector(m, l.wrapping_add(FORWARD))?,
            left: vector(m, l.wrapping_add(LEFT))?,
            up: vector(m, l.wrapping_add(UP))?,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_scene_hears_through_the_first_active_listener() {
        const G: u32 = 0x0060_0000;
        let mut g = FakeGuest::new();
        g.map(points::SOUND_GLOBALS.base & !0xfff, 0x1000).map(G, 0x1000);
        g.put_u32(points::SOUND_GLOBALS.base, G);
        assert!(ear(&g).unwrap().is_none(), "no listener active");
        let second = G + LISTENERS + LISTENER;
        g.put_u8(second + ACTIVE, 1);
        for (at, v) in [(FORWARD, [1.0, 0.0, 0.0]), (LEFT, [0.0, 1.0, 0.0]), (UP, [0.0, 0.0, 1.0]), (POSITION, [3.0, 4.0, 5.0])] {
            for (i, x) in v.into_iter().enumerate() {
                g.put_f32(second + at + 4 * i as u32, x);
            }
        }
        let e = ear(&g).unwrap().unwrap();
        assert_eq!((e.position, e.forward, e.left, e.up), ([3.0, 4.0, 5.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]));
    }
}
