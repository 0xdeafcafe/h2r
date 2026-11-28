//! The display's gamma ramp: what the brightness setting builds and the
//! present applies (docs/graphics/d3d-device.md, "Brightness and gamma").
//!
//! The game's view begin (`0x00012a50`, ported in `render_bridge::display`)
//! writes the ramp here; the presenting side reads it when it copies a frame
//! to the window. One process-wide ramp: the game has one display.

use std::sync::Mutex;

/// Bytes of a ramp: 256 red, 256 green, then 256 blue.
pub const RAMP_BYTES: usize = 768;

static RAMP: Mutex<Option<[u8; RAMP_BYTES]>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<[u8; RAMP_BYTES]>> {
    RAMP.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether every channel maps each level to itself.
pub fn is_identity(ramp: &[u8; RAMP_BYTES]) -> bool {
    ramp.chunks_exact(256).all(|c| c.iter().enumerate().all(|(i, &v)| usize::from(v) == i))
}

/// Keeps `ramp` as the display's (the brightness update's load).
pub fn set_ramp(ramp: [u8; RAMP_BYTES]) {
    *lock() = Some(ramp);
}

/// Forgets the ramp (a snapshot load of a run that had none).
pub fn clear_ramp() {
    *lock() = None;
}

/// The ramp last set, whatever it is.
pub fn ramp() -> Option<[u8; RAMP_BYTES]> {
    *lock()
}

/// The ramp the present applies: the last one set, `None` when there is
/// none or it maps every level to itself.
pub fn gamma() -> Option<[u8; RAMP_BYTES]> {
    ramp().filter(|r| !is_identity(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> [u8; RAMP_BYTES] {
        let mut r = [0u8; RAMP_BYTES];
        for (i, b) in r.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        r
    }

    #[test]
    fn the_identity_is_not_applied_and_a_curve_is() {
        // One test: the ramp is process-wide.
        clear_ramp();
        assert_eq!(gamma(), None, "none set");
        set_ramp(identity());
        assert_eq!(ramp(), Some(identity()));
        assert_eq!(gamma(), None, "the identity");
        let mut dim = identity();
        dim[300] = 7;
        set_ramp(dim);
        assert_eq!(gamma(), Some(dim));
        clear_ramp();
    }
}
