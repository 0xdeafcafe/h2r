//! Host replay state that is outside the machine snapshot. An entry keeps
//! indices into immutable film streams. Decoded ticks and call payloads are
//! read from the original film when a restore needs them.
use super::{Mode, Session};
use anyhow::{Context, Result, bail};
use film::tape::PlaybackCursor;

#[derive(Clone)]
pub(super) struct ReplayCursor {
    pub(super) tick: u64,
    pub(super) calls: u64,
    tape: PlaybackCursor,
    pending: bool,
    saw_actions: bool,
    last_pause: Option<bool>,
    pause_edges: Box<[bool]>,
}

impl ReplayCursor {
    pub(super) fn bytes(&self) -> usize {
        size_of::<Self>() + self.pause_edges.len()
    }
}

impl Session {
    pub(super) fn capture_replay_cursor(&self) -> Result<ReplayCursor> {
        let state = self.lock();
        let Mode::Replay(tape, _) = &state.mode else {
            bail!("a ring entry requires film playback");
        };
        let engine = state.engine.as_ref().context("missing film input track")?;
        Ok(ReplayCursor {
            tick: state.tick,
            calls: state.calls,
            tape: tape.cursor(),
            pending: engine.pending.is_some(),
            saw_actions: engine.saw_actions,
            last_pause: engine.last_pause,
            pause_edges: engine.pause_edges.clone().into_boxed_slice(),
        })
    }

    pub(super) fn restore_replay_cursor(&self, cursor: &ReplayCursor) -> Result<()> {
        let mut state = self.lock();
        let Mode::Replay(tape, _) = &mut state.mode else {
            bail!("a ring restore requires film playback");
        };
        tape.resume_cursor(cursor.tape)?;
        let engine = state.engine.as_mut().context("missing film input track")?;
        engine.track.resume_after_tick(cursor.tick)?;
        engine.pending = None;
        if cursor.pending {
            engine.next_tick()?;
        }
        engine.saw_actions = cursor.saw_actions;
        engine.last_pause = cursor.last_pause;
        engine.pause_edges = cursor.pause_edges.to_vec();
        state.tick = cursor.tick;
        state.calls = cursor.calls;
        state.complete = false;
        state.failure = None;
        Ok(())
    }
}
