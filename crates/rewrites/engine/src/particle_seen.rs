//! Particles age as if seen (docs/engine/sim-render-split.md, "6. Render
//! scratch and stamps"). The director's per-frame move of a particle
//! location (`0x00248750`, EBX the location) walks its emitters through
//! `0x002483b0` only when the location's stamp `+0x30` plus one is the
//! render frame counter (`0x00248870..0x0024887b`), and the stamp is the
//! render's: `0x00174420` writes it for each particle a view keeps. So
//! what a view drew chose how a particle system moved.
//!
//! [`as_seen`] runs at the move's entry and sets the stamp to the last
//! frame for every location, drawn or not, so every system takes the seen
//! path whatever the render did, and the render's own stamps stay out of
//! the arena (`render_bridge::effects_shadow::stamps`).

use guest_layout::memory::{GuestMemory, MemFault};

/// The render frame counter (`render_views`).
pub const FRAME: u32 = 0x004b_a034;
/// The location's frame stamp.
pub const STAMP: u32 = 0x30;

/// At `0x00248750`'s entry: the stamp of `location` that the move reads as
/// seen last frame.
pub fn as_seen(m: &mut dyn GuestMemory, location: u32) -> Result<(), MemFault> {
    let frame = m.u32(FRAME)?;
    m.set_u32(location.wrapping_add(STAMP), frame.wrapping_sub(1))
}

/// Whether the move at `0x00248870` takes the seen path for `location`.
pub fn seen(m: &dyn GuestMemory, location: u32) -> Result<bool, MemFault> {
    Ok(m.u32(location.wrapping_add(STAMP))?.wrapping_add(1) == m.u32(FRAME)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const SEEN: u32 = 0x0060_0000;
    const UNSEEN: u32 = 0x0060_0040;

    #[test]
    fn a_seen_and_an_unseen_system_age_alike() {
        let mut m = FlatMemory::new(0x004b_0000, 0x0020_0000);
        m.set_u32(FRAME, 500).unwrap();
        // The view kept one location last frame and never drew the other.
        m.set_u32(SEEN + STAMP, 499).unwrap();
        m.set_u32(UNSEEN + STAMP, 12).unwrap();
        assert!(seen(&m, SEEN).unwrap());
        assert!(!seen(&m, UNSEEN).unwrap());
        for at in [SEEN, UNSEEN] {
            as_seen(&mut m, at).unwrap();
        }
        assert!(seen(&m, SEEN).unwrap() && seen(&m, UNSEEN).unwrap());
        assert_eq!(m.u32(SEEN + STAMP).unwrap(), m.u32(UNSEEN + STAMP).unwrap(), "the move reads the same stamp");
        // The first frame of a session.
        m.set_u32(FRAME, 0).unwrap();
        as_seen(&mut m, UNSEEN).unwrap();
        assert!(seen(&m, UNSEEN).unwrap());
    }

    #[test]
    fn a_location_off_the_map_faults() {
        let mut m = FlatMemory::new(0x004b_0000, 0x0010_0000);
        assert!(as_seen(&mut m, 0x0100_0000).is_err());
    }
}
