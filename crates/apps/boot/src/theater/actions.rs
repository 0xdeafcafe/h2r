//! A `game_tick` builds one retail 0x4048-byte simulation update on its
//! stack. By entry to SIM_UPDATE_APPLY, input gathering, remote arrivals
//! and phantom slot actions have already been resolved. The mask is by
//! player datum index, including split-screen players, not by pad port.
//! See docs/live/live-coop.md §§1.3 and "Visual prediction" and
//! coop_look::State::applied, which reads facing from these same records.
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use engine::GuestMemory;
use film::blam::{MAX_SEATS, SeatInput};

use super::{Mode, Session};

const MASK: u32 = 8;
const ACTION_BYTES: usize = 0x5c;
// From the mask at +8 to the end of the final action, immediately
// before the player-state mask at +0x5cc. No other update fields change.
const ACTION_BLOCK: usize = 4 + ACTION_BYTES * MAX_SEATS;

fn block_address(update: u32) -> Result<u32> {
    ensure!(update != 0, "null film simulation update");
    update
        .checked_add(MASK + ACTION_BLOCK as u32 - 1)
        .context("film simulation actions wrap guest memory")?;
    Ok(update + MASK)
}

fn read_block(m: &dyn GuestMemory, at: u32) -> Result<[u8; ACTION_BLOCK]> {
    let mut block = [0; ACTION_BLOCK];
    m.read(at, &mut block)
        .map_err(|_| anyhow::anyhow!("cannot read film simulation actions at {at:#x}"))?;
    Ok(block)
}

fn capture(
    m: &dyn GuestMemory,
    update: u32,
    pads: &[Option<input::Gamepad>; MAX_SEATS],
) -> Result<Vec<SeatInput>> {
    let block = read_block(m, block_address(update)?)?;
    let mask = u32::from_le_bytes(block[..4].try_into()?);
    ensure!(
        mask >> MAX_SEATS == 0,
        "invalid film player-action mask {mask:#x}"
    );
    let mut inputs = Vec::with_capacity(mask.count_ones() as usize);
    for (seat, record) in block[4..].chunks_exact(ACTION_BYTES).enumerate() {
        if mask & (1 << seat) != 0 {
            inputs.push(SeatInput {
                seat: seat as u8,
                action: Some(record.try_into()?),
                pad: pads[seat].map(|pad| pad.to_bytes()),
            });
        }
    }
    Ok(inputs)
}

fn apply(m: &mut dyn GuestMemory, update: u32, inputs: &[SeatInput]) -> Result<()> {
    let at = block_address(update)?;
    let mut block = read_block(m, at)?;
    let mut changed = false;
    let mut mask = 0u32;
    for input in inputs {
        let Some(record) = &input.action else {
            continue;
        };
        let seat = usize::from(input.seat);
        ensure!(
            seat < MAX_SEATS && mask & (1 << seat) == 0,
            "invalid film action seat {seat}"
        );
        mask |= 1 << seat;
        let start = 4 + seat * ACTION_BYTES;
        let dest = &mut block[start..start + ACTION_BYTES];
        changed |= *dest != record[..];
        dest.copy_from_slice(record);
    }
    if changed || block[..4] != mask.to_le_bytes() {
        block[..4].copy_from_slice(&mask.to_le_bytes());
        m.write(at, &block)
            .map_err(|_| anyhow::anyhow!("cannot write film simulation actions at {at:#x}"))?;
    }
    Ok(())
}

pub(super) fn install(b: &mut hooks::Builder, session: Arc<Session>) {
    b.raw().before(
        &engine::points::SIM_UPDATE_APPLY,
        "film simulation actions",
        move |c| {
            let mut state = session.lock();
            if state.complete || state.failure.is_some() {
                return Ok(hooks::Flow::Continue);
            }
            let update = c
                .arg_u32("update")
                .context("cannot read film simulation update argument");
            let mut memory = c.ctx.memory();
            let result = match update {
                Err(e) => Err(e),
                Ok(ptr) if matches!(&state.mode, Mode::Record(_)) => {
                    let pads = bots::guest::game_tick(&*memory)
                        .map(|tick| bots::phantom::global().gathered_pads(tick))
                        .unwrap_or([None; MAX_SEATS]);
                    match capture(&*memory, ptr, &pads) {
                        Ok(inputs) => {
                            drop(memory);
                            drop(state);
                            session.record_actions(inputs);
                            return Ok(hooks::Flow::Continue);
                        }
                        Err(e) => Err(e),
                    }
                }
                Ok(ptr) => (|| -> Result<()> {
                    let engine = state
                        .engine
                        .as_mut()
                        .context("missing film command track")?;
                    engine.note_actions()?;
                    apply(&mut *memory, ptr, &engine.next_tick()?.inputs)
                })(),
            };
            if let Err(e) = result {
                let tick = state.tick + 1;
                state.fail_at(tick, e.to_string());
            }
            Ok(hooks::Flow::Continue)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn gathered_actions_follow_sparse_player_slots_and_replay_ignores_pad_only_rows() {
        let point = &engine::points::SIM_UPDATE_APPLY;
        assert_eq!(point.sig.arg("update"), Some(engine::points::Loc::Stack(0)));
        let mut mem = FlatMemory::new(0x8000, 0x4048);
        let update = mem.base;
        mem.bytes.fill(0xa5);
        let mut actor = [0; ACTION_BYTES];
        actor[0] = 2; // jump flag
        actor[8..12].copy_from_slice(&0.25f32.to_le_bytes());
        actor[ACTION_BYTES - 1] = 0xef;
        let players = [
            SeatInput {
                seat: 0,
                action: Some([0; ACTION_BYTES]),
                pad: None,
            },
            SeatInput {
                seat: 7,
                action: None,
                pad: Some([0; 18]),
            },
            SeatInput {
                seat: 15,
                action: Some(actor),
                pad: None,
            },
        ];
        apply(&mut mem, update, &players).unwrap();
        assert_eq!(
            capture(&mem, update, &[None; MAX_SEATS]).unwrap(),
            vec![players[0], players[2]]
        );
        // The phantom gate reported an accepted neutral pad for seat 15.
        // A human in slot zero has only an authoritative command here.
        let mut pads = [None; MAX_SEATS];
        pads[15] = Some(input::Gamepad::default());
        pads[7] = Some(input::Gamepad::default()); // No published retail action.
        let rows = capture(&mem, update, &pads).unwrap();
        assert_eq!(
            rows,
            vec![
                players[0],
                SeatInput {
                    pad: Some([0; 18]),
                    ..players[2]
                }
            ]
        );
        assert_eq!(mem.u32(update + MASK).unwrap(), 0x8001);
        assert!(mem.bytes[..8].iter().all(|v| *v == 0xa5));
        assert!(
            mem.bytes[12 + ACTION_BYTES..12 + 15 * ACTION_BYTES]
                .iter()
                .all(|v| *v == 0xa5)
        );
        assert!(mem.bytes[0x5cc..].iter().all(|v| *v == 0xa5));
    }

    #[test]
    fn invalid_seat_or_truncated_memory_leaves_the_update_intact() {
        let mut mem = FlatMemory::new(32, 0x4048);
        let before = mem.bytes.clone();
        let good = SeatInput {
            seat: 2,
            action: Some([1; ACTION_BYTES]),
            pad: None,
        };
        let bad = SeatInput { seat: 16, ..good };
        for inputs in [[good, bad], [good, good]] {
            assert!(apply(&mut mem, 32, &inputs).is_err());
        }
        assert!(apply(&mut mem, u32::MAX - 24, &[good]).is_err());
        assert!(apply(&mut mem, 32 + 0x4044, &[good]).is_err());
        assert_eq!(mem.bytes, before);
        mem.set_u32(32 + MASK, 1 << 16).unwrap();
        assert!(capture(&mem, 32, &[None; MAX_SEATS]).is_err());
    }
}
