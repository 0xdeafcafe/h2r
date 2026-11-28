//! A `game_tick` builds one retail 0x4048-byte simulation update on its
//! stack. By entry to SIM_UPDATE_APPLY, input gathering, remote arrivals
//! and phantom slot actions have already been resolved. The mask is by
//! player datum index, including split-screen players, not by pad port.
//! The record's head (`+0`, the update number) and stamp (`+0x4040`, the
//! tick count; `+0x4044`, the random word) are the engine's own and are
//! never written. Between them are the action block (`+8`) and the body
//! (`+0x5cc` through `+0x403f`: player entries, the player-set change, the
//! event list, the save flag), which the film stores as the bytes that
//! changed since the update before (docs/game/theater-design.md, "The
//! update stream").
//! See docs/live/live-coop.md §§1.3 and "Visual prediction" and
//! coop_look::State::applied, which reads facing from these same records.
use std::sync::{Arc, MutexGuard};

use anyhow::{Context, Result, ensure};
use guest_layout::GuestMemory;
use film::blam::{MAX_SEATS, SeatInput};

use super::{Mode, Session, State, film_play, game_start};

const MASK: u32 = 8;
const ACTION_BYTES: usize = 0x5c;
// From the mask at +8 to the end of the final action, immediately
// before the player-state mask at +0x5cc. No other update fields change.
const ACTION_BLOCK: usize = 4 + ACTION_BYTES * MAX_SEATS;
/// The body starts where the action block ends: the player-state mask.
const BODY_AT: u32 = MASK + ACTION_BLOCK as u32;
/// The stamp: the tick count the update was built for.
const STAMP_AT: u32 = 0x4040;
const BODY_BYTES: usize = (STAMP_AT - BODY_AT) as usize;
const _: () = assert!(BODY_BYTES == film::columns::BODY_BYTES);

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

fn body_address(update: u32) -> Result<u32> {
    ensure!(update != 0, "null film simulation update");
    update
        .checked_add(STAMP_AT + 8)
        .context("film simulation update wraps guest memory")?;
    Ok(update + BODY_AT)
}

/// The update's body, between its action block and its stamp.
pub(super) fn capture_body(m: &dyn GuestMemory, update: u32) -> Result<Vec<u8>> {
    let at = body_address(update)?;
    let mut body = vec![0; BODY_BYTES];
    m.read(at, &mut body)
        .map_err(|_| anyhow::anyhow!("cannot read film simulation body at {at:#x}"))?;
    Ok(body)
}

/// The tick count the update was stamped with.
pub(super) fn capture_stamp(m: &dyn GuestMemory, update: u32) -> Result<u32> {
    body_address(update)?;
    let at = update + STAMP_AT;
    let mut stamp = [0; 4];
    m.read(at, &mut stamp)
        .map_err(|_| anyhow::anyhow!("cannot read film simulation stamp at {at:#x}"))?;
    Ok(u32::from_le_bytes(stamp))
}

/// Whether an update stamped `stamp` may follow the one stamped `last`
/// (`None` at a game's first): the same tick again, the next, or an
/// earlier one (a campaign revert puts the clock back). A stamp that is
/// further on than the next means an update the film does not hold.
pub(super) fn stamp_follows(last: Option<u32>, stamp: u32) -> bool {
    last.is_none_or(|last| stamp <= last.saturating_add(1))
}

/// Write a recorded body over the one the engine built. Writes nothing
/// when they agree, and nothing but the body when they do not.
pub(super) fn apply_body(m: &mut dyn GuestMemory, update: u32, body: &[u8]) -> Result<()> {
    ensure!(
        body.len() == BODY_BYTES,
        "film simulation body is {} bytes, not {BODY_BYTES}",
        body.len()
    );
    let at = body_address(update)?;
    let mut current = vec![0; BODY_BYTES];
    m.read(at, &mut current)
        .map_err(|_| anyhow::anyhow!("cannot read film simulation body at {at:#x}"))?;
    if current != body {
        m.write(at, body)
            .map_err(|_| anyhow::anyhow!("cannot write film simulation body at {at:#x}"))?;
    }
    Ok(())
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

pub(super) fn apply(m: &mut dyn GuestMemory, update: u32, inputs: &[SeatInput]) -> Result<()> {
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

/// The film's facing for each local user, written to its look state on
/// the update ([`crate::coop_look::follow_film`]), when the picture is a
/// first-person view. Every other camera leaves the look alone.
pub(super) fn apply_look(m: &mut dyn GuestMemory, update: u32, first_person: bool) -> usize {
    if first_person {
        crate::coop_look::follow_film(m, update)
    } else {
        0
    }
}

/// A game's first update: take the start the activation left waiting, with
/// the random words and the arena hash as they are at entry to this update,
/// and hold it until the tick's observer has described the game
/// ([`game_start::Starts::release`]). A default recording's film counts its
/// ticks from here: this update is its tick 1. A game the observer has not
/// described is taken again at the next update. A recording with no film
/// open keeps no start. A start that cannot be read says so and the
/// recording goes on.
pub(super) fn note_first_update(
    store: Option<&Arc<settings::Store>>,
    state: &mut State,
    memory: &runtime::Guest,
) {
    if !matches!(&state.mode, Mode::Record(rec) if rec.starts.waiting()) {
        return;
    }
    let header = state.header.clone();
    let session_tick = state.tick;
    let Mode::Record(rec) = &mut state.mode else {
        return;
    };
    let Some(options) = rec.starts.take_options() else {
        return;
    };
    // The game's own tick count begins here.
    if !rec.starts.retaking() {
        rec.frame.last_stamp = None;
    }
    let Some(game) = rec.game.as_mut() else {
        return;
    };
    if game.base.is_none() {
        game.base = Some(session_tick);
        // A game described before its first update starts at its film's
        // tick 1.
        rec.meta.begin_at(1);
    }
    let tick = game.film.ticks() + 1;
    match game_start::first_update(memory, options, &header, store.map(|s| &**s), tick) {
        Ok(start) => rec.starts.hold(start),
        Err(error) => eprintln!("theater: this game has no start in its film: {error:#}"),
    }
}

/// An update entered `SIM_UPDATE_APPLY`. A recording takes the actions the
/// engine built for the tick; a film played from its game start writes the
/// film's over them ([`film_play::update`]); a machine film writes its
/// track's. `state` is the session's lock, held from before the guest's
/// memory was taken.
pub(super) fn at_update(
    session: &Session,
    mut state: MutexGuard<'_, State>,
    memory: &mut runtime::Guest,
    update: Result<u32>,
) {
    let result = match update {
        Err(e) => Err(e),
        Ok(ptr) if matches!(&state.mode, Mode::Record(_)) => {
            note_first_update(session.video_menu.get(), &mut state, memory);
            let pads = bots::guest::game_tick(&*memory)
                .map(|tick| bots::phantom::global().gathered_pads(tick))
                .unwrap_or([None; MAX_SEATS]);
            let captured = capture(&*memory, ptr, &pads).and_then(|inputs| {
                Ok((
                    inputs,
                    capture_body(&*memory, ptr)?,
                    capture_stamp(&*memory, ptr)?,
                ))
            });
            match captured {
                Ok((inputs, body, stamp)) => {
                    drop(state);
                    session.record_update(inputs, Some(body), Some(stamp));
                    return;
                }
                Err(e) => Err(e),
            }
        }
        Ok(ptr) => film_play::update(&mut state, memory, ptr),
    };
    if let Err(e) = result {
        let tick = state.tick + 1;
        state.fail_at(tick, e.to_string());
    }
}

pub(super) fn install(b: &mut hooks::Builder, session: Arc<Session>) {
    b.raw().before(
        &guest_layout::points::SIM_UPDATE_APPLY,
        "film simulation actions",
        move |c| {
            let state = session.lock();
            if state.complete || state.failure.is_some() {
                return Ok(hooks::Flow::Continue);
            }
            let update = c
                .arg_u32("update")
                .context("cannot read film simulation update argument");
            let mut memory = c.ctx.memory();
            at_update(&session, state, &mut memory, update);
            Ok(hooks::Flow::Continue)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;

    #[test]
    fn a_camera_that_is_not_first_person_leaves_the_look_alone() {
        let mut mem = FlatMemory::new(0x8000, 0x4048);
        let update = mem.base;
        mem.bytes.fill(0x3c);
        let before = mem.bytes.clone();
        assert_eq!(apply_look(&mut mem, update, false), 0);
        assert_eq!(mem.bytes, before);
        // First person with no local subject in this memory writes nothing.
        assert_eq!(apply_look(&mut mem, update, true), 0);
        assert_eq!(mem.bytes, before);
    }

    #[test]
    fn gathered_actions_follow_sparse_player_slots_and_replay_ignores_pad_only_rows() {
        let point = &guest_layout::points::SIM_UPDATE_APPLY;
        assert_eq!(point.sig.arg("update"), Some(guest_layout::points::Loc::Stack(0)));
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

    #[test]
    fn capture_and_apply_leave_the_head_and_stamp_untouched() {
        let mut mem = FlatMemory::new(0x8000, 0x4048);
        let update = mem.base;
        mem.bytes.fill(0xa5);
        let head: Vec<u8> = (0..8).map(|b| 0x10 + b).collect();
        let stamp: Vec<u8> = (0..8).map(|b| 0xc0 + b).collect();
        mem.bytes[..8].copy_from_slice(&head);
        mem.bytes[0x4040..].copy_from_slice(&stamp);
        assert_eq!(BODY_AT, 0x5cc);
        assert_eq!(BODY_BYTES, 0x4040 - 0x5cc);
        assert_eq!(
            capture_stamp(&mem, update).unwrap(),
            u32::from_le_bytes(stamp[..4].try_into().unwrap())
        );

        // A body with a player-set change, an event and the save flag.
        let mut body = vec![0u8; BODY_BYTES];
        body[0xdd0 - 0x5cc..0xdd0 - 0x5cc + 4].copy_from_slice(&7u32.to_le_bytes());
        body[0xe38 - 0x5cc..0xe38 - 0x5cc + 4].copy_from_slice(&1u32.to_le_bytes());
        body[0x403c - 0x5cc] = 1;
        let seat = SeatInput { seat: 3, action: Some([9; ACTION_BYTES]), pad: None };
        apply(&mut mem, update, &[seat]).unwrap();
        apply_body(&mut mem, update, &body).unwrap();
        assert_eq!(capture_body(&mem, update).unwrap(), body);
        assert_eq!(capture(&mem, update, &[None; MAX_SEATS]).unwrap(), vec![seat]);
        assert_eq!(mem.bytes[..8], head[..]);
        assert_eq!(mem.bytes[0x4040..], stamp[..]);
        // The action block and the body meet and neither writes the other's.
        assert_eq!(mem.u32(update + 0xdd0).unwrap(), 7);
        assert_eq!(mem.bytes[0x403c], 1);
        assert_eq!(mem.u32(update + MASK).unwrap(), 1 << 3);

        // A body of the wrong size, or on memory that ends early, writes nothing.
        let before = mem.bytes.clone();
        assert!(apply_body(&mut mem, update, &body[1..]).is_err());
        assert_eq!(mem.bytes, before);
        let mut short = FlatMemory::new(32, 0x4000);
        assert!(capture_body(&short, 32).is_err());
        assert!(capture_stamp(&short, 32).is_err());
        assert!(apply_body(&mut short, 32, &body).is_err());
        assert!(capture_body(&mem, u32::MAX - 100).is_err());
    }
}
