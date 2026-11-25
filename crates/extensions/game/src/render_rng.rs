//! The render's private random stream (docs/graphics/high-resolution.md,
//! "Determinism"). The render front-end steps the shared random seed
//! (`[0x004e7408]+0`, object code `0x001032a0` reached from the render),
//! so what a frame draws would choose the simulation's next random
//! numbers. While a world frame renders, the seed holds a private stream
//! of its own, and the simulation's seed is put back when the frame ends:
//! the simulation's stream then advances only by the simulation's draws,
//! whatever the view draws.
//!
//! Two features enter it: Theater's camera, while it changes what a
//! replay draws (`theater/camera.rs`), and [`RenderRng`], every world frame
//! of a session whose guest camera can take the window's aspect. Both use
//! the one process stream ([`shared`]); a second enter in the same frame
//! is a no-op, so the frame is entered once and left once.

use std::sync::{Arc, Mutex, OnceLock};

use engine::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest};

pub const ID: &str = "render-rng";
/// Theater's camera feature (`theater::camera::FEATURE`), which enters the
/// same stream: ordered after it, so the two hooks at `RENDER_FRAME` do not
/// clash.
const THEATER_CAMERA: &str = "theater-camera";

/// The shared seed's save and the private stream.
#[derive(Default)]
pub(crate) struct Seeds {
    restore: Option<(u32, [u8; 8])>,
    private: Option<[u8; 8]>,
}

impl Seeds {
    /// Put the private stream in the seed, keeping the simulation's for
    /// [`Seeds::leave`]. The first enter starts the private stream from
    /// the simulation's seed. Entered already: nothing.
    pub(crate) fn enter(&mut self, guest: &mut dyn GuestMemory) {
        if self.restore.is_some() {
            return;
        }
        let Ok(addr) = guest.u32(crate::sim_glue::RANDOM_SEED_PTR) else {
            return;
        };
        if addr == 0 {
            return;
        }
        let mut original = [0; 8];
        if guest.read(addr, &mut original).is_ok()
            && guest
                .write(addr, self.private.as_ref().unwrap_or(&original))
                .is_ok()
        {
            self.restore = Some((addr, original));
        }
    }

    /// Keep where the private stream got to and put the simulation's seed
    /// back. Not entered: nothing.
    pub(crate) fn leave(&mut self, guest: &mut dyn GuestMemory) {
        let Some((addr, original)) = self.restore.take() else {
            return;
        };
        let mut advanced = [0; 8];
        if guest.read(addr, &mut advanced).is_ok() {
            self.private = Some(advanced);
        }
        let _ = guest.write(addr, &original);
    }
}

/// The process's render stream, shared by every feature that enters it.
pub(crate) fn shared() -> Arc<Mutex<Seeds>> {
    static SHARED: OnceLock<Arc<Mutex<Seeds>>> = OnceLock::new();
    SHARED.get_or_init(Default::default).clone()
}

/// The players' globals and their bitmask of valid machines (`+0x2c`;
/// docs/game/bots.md, "Players globals").
const PLAYER_GLOBALS: u32 = 0x004e_8c20;
const VALID_MACHINES: u32 = 0x2c;

/// A game of more than one machine (another console, or a bot's phantom
/// machine): its peers simulate the same world through their own retail
/// cameras, so it keeps the retail lens and the retail random stream.
/// Unreadable globals count as networked; none yet (the menus) does not.
pub(crate) fn networked(mem: &dyn GuestMemory) -> bool {
    let Ok(globals) = mem.u32(PLAYER_GLOBALS) else { return true };
    if globals == 0 {
        return false;
    }
    match globals.checked_add(VALID_MACHINES).map(|at| mem.u32(at)) {
        Some(Ok(mask)) => mask.count_ones() > 1,
        _ => true,
    }
}

/// Whether a frame takes the private stream: a world frame of a game on
/// one machine.
fn private_frame(mem: &dyn GuestMemory) -> bool {
    sim_bridge::sim_frame::frame_kind(&sim_bridge::sim_frame::Addrs::HALO2, mem)
        == sim_bridge::sim_frame::FrameKind::World
        && !networked(mem)
}

/// Every world frame of a one-machine game renders on the private stream.
/// Registered by boot for every session that renders, except lockstep and
/// input tapes; the same rule on the recording and the replay, so a film
/// replays whatever window either had.
pub struct RenderRng;

impl Feature for RenderRng {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The render's private random stream").doc("docs/graphics/high-resolution.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
        let seeds = shared();
        let entering = seeds.clone();
        b.raw()
            .before(&points::RENDER_FRAME, "enter the render's random stream", move |call| {
                let mut guest = call.ctx.memory();
                if private_frame(&*guest) {
                    entering.lock().unwrap_or_else(|e| e.into_inner()).enter(&mut *guest);
                }
                Ok(Flow::Continue)
            })
            .after_feature(crate::bot_glue::ID)
            .after_feature(THEATER_CAMERA);
        b.raw()
            .after(&points::RENDER_FRAME, "restore the simulation's random stream", move |call| {
                seeds.lock().unwrap_or_else(|e| e.into_inner()).leave(&mut *call.ctx.memory());
                Ok(())
            })
            .after_feature(crate::bot_glue::ID)
            .after_feature(THEATER_CAMERA);
        Ok(())
    }
}

pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry.add(&RenderRng).map_err(|e| anyhow::anyhow!("render rng: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const SEED: u32 = 0x0060_0000;

    fn guest(seed: [u8; 8]) -> FakeGuest {
        let mut mem = FakeGuest::new();
        mem.put_u32(crate::sim_glue::RANDOM_SEED_PTR, SEED).put(SEED, &seed);
        mem
    }

    fn seed(mem: &FakeGuest) -> [u8; 8] {
        let mut out = [0; 8];
        mem.read(SEED, &mut out).unwrap();
        out
    }

    #[test]
    fn a_frame_on_the_private_stream_leaves_the_simulations_seed_as_it_was() {
        let mut mem = guest([1; 8]);
        let mut seeds = Seeds::default();
        seeds.enter(&mut mem);
        assert_eq!(seed(&mem), [1; 8], "the first frame starts from the simulation's seed");
        mem.put(SEED, &[7; 8]); // the render steps it
        seeds.leave(&mut mem);
        assert_eq!(seed(&mem), [1; 8]);

        // The simulation moves on; the next frame continues the private stream.
        mem.put(SEED, &[2; 8]);
        seeds.enter(&mut mem);
        assert_eq!(seed(&mem), [7; 8]);
        seeds.leave(&mut mem);
        assert_eq!(seed(&mem), [2; 8]);
    }

    #[test]
    fn two_features_entering_one_frame_enter_and_leave_once() {
        let mut mem = guest([3; 8]);
        let mut seeds = Seeds::default();
        seeds.enter(&mut mem); // Theater's camera
        mem.put(SEED, &[9; 8]);
        seeds.enter(&mut mem); // the render stream: already entered
        assert_eq!(seed(&mem), [9; 8], "a second enter does not save the render's seed");
        seeds.leave(&mut mem);
        seeds.leave(&mut mem); // the second leave: nothing
        assert_eq!(seed(&mem), [3; 8]);
    }

    #[test]
    fn a_game_of_several_machines_keeps_the_simulations_stream() {
        let mut mem = FakeGuest::new();
        mem.put_u32(PLAYER_GLOBALS, 0);
        assert!(!networked(&mem), "no players yet");
        mem.put_u32(PLAYER_GLOBALS, 0x1000).put_u32(0x1000 + VALID_MACHINES, 0b1);
        assert!(!networked(&mem), "one machine");
        mem.put_u32(0x1000 + VALID_MACHINES, 0b11);
        assert!(networked(&mem), "a peer or a bot's phantom machine");
        assert!(networked(&FakeGuest::new()), "unreadable");
    }
}
