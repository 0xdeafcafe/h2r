//! The player's answer to the engine's per-frame tick count. The engine
//! asks once a frame how many ticks to run (`GAME_UPDATE_TICKS`, `eax`);
//! while a film plays under guest pacing the answer is the transport's:
//! none in a pause, the ticks to a step's or a jump's target, the ticks a
//! frame is worth at the chosen speed. The hook writes the count into the
//! register and the frame's elapsed seconds beside it, and nothing else, so
//! no game state moves and the digests do not change
//! (docs/game/theater-design.md "Pause, speed and step").
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use engine::GuestMemory;

use super::{Mode, Session};

/// Writes the answer's seconds to the engine's elapsed output.
fn answer(
    m: &mut dyn GuestMemory,
    eax: &mut u32,
    elapsed: u32,
    ticks: u32,
    tick_hz: f64,
) -> Result<()> {
    if *eax == ticks {
        return Ok(());
    }
    *eax = ticks;
    let seconds = (f64::from(ticks) / tick_hz) as f32;
    m.write(elapsed, &seconds.to_le_bytes())
        .map_err(|_| anyhow::anyhow!("cannot write the frame's elapsed time at {elapsed:#x}"))
}

pub(super) fn install(b: &mut hooks::Builder, session: Arc<Session>) {
    b.raw().before(
        &engine::points::GAME_UPDATE_TICKS,
        "film pacing",
        move |c| {
            let mut state = session.lock();
            if state.complete || state.failure.is_some() || !matches!(state.mode, Mode::Replay(..)) {
                return Ok(hooks::Flow::Continue);
            }
            let demand = state.demand();
            let Some(clock) = state.pacing.as_mut() else {
                return Ok(hooks::Flow::Continue);
            };
            let Some(ticks) = clock.frame_ticks(demand, c.cpu.eax, Instant::now()) else {
                return Ok(hooks::Flow::Continue);
            };
            drop(state);
            let mut memory = c.ctx.memory();
            let mut eax = c.cpu.eax;
            let result = answer(&mut *memory, &mut eax, c.cpu.ebx, ticks, demand.tick_hz);
            drop(memory);
            c.cpu.eax = eax;
            if let Err(e) = result {
                let mut state = session.lock();
                let tick = state.tick + 1;
                state.fail_at(tick, e.to_string());
            }
            Ok(hooks::Flow::Continue)
        },
    );
}

impl Session {
    /// From here the player answers the guest's tick count. A film that
    /// starts from its game start plays this way; it takes no other pacing.
    pub(super) fn pace_in_guest(&self) {
        self.lock().pacing.get_or_insert_with(Default::default);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn the_answer_changes_the_count_and_the_elapsed_seconds_and_nothing_else() {
        let mut mem = FlatMemory::new(0x8000, 0x100);
        mem.bytes.fill(0xa5);
        let before = mem.bytes.clone();
        let elapsed = mem.base + 0x40;
        let mut eax = 3;
        answer(&mut mem, &mut eax, elapsed, 0, 30.0).unwrap();
        assert_eq!(eax, 0);
        assert_eq!(mem.f32(elapsed), Ok(0.0));
        let changed: Vec<_> = (0..mem.bytes.len())
            .filter(|&i| mem.bytes[i] != before[i])
            .collect();
        assert_eq!(changed, vec![0x40, 0x41, 0x42, 0x43]);

        answer(&mut mem, &mut eax, elapsed, 4, 30.0).unwrap();
        assert_eq!((eax, mem.f32(elapsed)), (4, Ok(4.0 / 30.0)));
    }

    #[test]
    fn an_answer_equal_to_the_engines_touches_nothing() {
        let mut mem = FlatMemory::new(0x8000, 0x100);
        let before = mem.bytes.clone();
        let mut eax = 2;
        answer(&mut mem, &mut eax, mem.base, 2, 30.0).unwrap();
        assert_eq!((eax, mem.bytes), (2, before));
    }
}
