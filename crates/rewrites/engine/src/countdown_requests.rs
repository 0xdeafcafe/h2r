//! Tick phase 29, `0x0016f200`, in Rust (docs/engine/sim-rewrite.md, "The
//! tick driver"): the two one-shot requests that restart the frame
//! countdowns the render reads before it presents
//! ([`crate::view_records::COUNTDOWN`], [`crate::view_records::SPLIT_COUNTDOWN`]).
//!
//! The guest body, whole (`0x16f200..0x16f275`, no calls):
//!
//! | Request byte | Unless `[[0x4e6948] + 8] == 2` | Then |
//! |---|---|---|
//! | `0x4ea935` set | the countdowns `+0`, `+0x30` = 3, their flags `+4`, `+0x34` = 1, `+0x3c` = 0 (block `0x4e6470`) | the byte cleared |
//! | `0x4ea936` set | the same with the flags 0 | the byte cleared |
//!
//! The camera override's kind is read only for a set byte, as the guest
//! does. The phase's registers are scratch to the tick driver, so none is
//! reproduced. `countdown_requests_match_lifted`
//! (`tests/sim_seats_differential.rs`) holds it to the lifted body.

use crate::memory::{GuestMemory, MemFault};
use crate::view_records::{CAMERA_OVERRIDE, COUNTDOWN};

/// The phase's guest entry.
pub const ENTRY: u32 = 0x0016_f200;
/// The request that restarts the countdowns with their flags set.
pub const FIRST: u32 = 0x004e_a935;
/// The request that restarts them with their flags clear.
pub const SECOND: u32 = 0x004e_a936;
/// The camera override kind that leaves the countdowns alone.
const HELD: u32 = 2;
/// Frames each countdown restarts at.
const FRAMES: u32 = 3;

/// Run the phase on `m`. A fault is the guest's own: the same read faults
/// in the lifted body.
pub fn run(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    for (request, flag) in [(FIRST, 1), (SECOND, 0)] {
        if m.u8(request)? == 0 {
            continue;
        }
        let camera = m.u32(CAMERA_OVERRIDE)?;
        if m.u32(camera.wrapping_add(8))? != HELD {
            for (offset, value) in [(0x34, flag), (0x30, FRAMES), (0x3c, 0), (0, FRAMES), (4, flag)] {
                m.set_u32(COUNTDOWN + offset, value)?;
            }
        }
        m.set_u8(request, 0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    const CAMERA: u32 = 0x0060_0000;

    fn guest(first: u8, second: u8, kind: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x004e_6000, 0x5000).map(CAMERA, 0x1000);
        g.put_u32(CAMERA_OVERRIDE, CAMERA).put_u32(CAMERA + 8, kind);
        g.put_u8(FIRST, first).put_u8(SECOND, second);
        for o in (0..0x40).step_by(4) {
            g.put_u32(COUNTDOWN + o, 0x5555_0000 + o);
        }
        g
    }

    fn block(g: &FakeGuest) -> Vec<u32> {
        (0..0x40).step_by(4).map(|o| g.u32(COUNTDOWN + o).unwrap()).collect()
    }

    #[test]
    fn nothing_pending_writes_nothing() {
        let mut g = guest(0, 0, 0);
        let before = block(&g);
        run(&mut g).unwrap();
        assert_eq!(block(&g), before);
    }

    #[test]
    fn each_request_restarts_the_countdowns_and_clears_itself() {
        for (first, second, flag) in [(1, 0, 1), (0, 7, 0), (1, 1, 0)] {
            let mut g = guest(first, second, 0);
            run(&mut g).unwrap();
            let b = block(&g);
            assert_eq!((b[0], b[1], b[12], b[13], b[15]), (3, flag, 3, flag, 0), "{first} {second}");
            assert_eq!(b[2], 0x5555_0008, "the words between are untouched");
            assert_eq!((g.u8(FIRST).unwrap(), g.u8(SECOND).unwrap()), (0, 0));
        }
    }

    #[test]
    fn a_held_camera_only_clears_the_requests() {
        let mut g = guest(1, 1, HELD);
        let before = block(&g);
        run(&mut g).unwrap();
        assert_eq!(block(&g), before);
        assert_eq!((g.u8(FIRST).unwrap(), g.u8(SECOND).unwrap()), (0, 0));
    }

    #[test]
    fn the_camera_is_read_only_for_a_request() {
        let mut g = FakeGuest::new();
        g.map(0x004e_6000, 0x5000);
        g.put_u32(CAMERA_OVERRIDE, 0xdead_0000);
        run(&mut g).unwrap();
        g.put_u8(FIRST, 1);
        assert!(run(&mut g).is_err(), "the guest faults on the same read");
    }
}
